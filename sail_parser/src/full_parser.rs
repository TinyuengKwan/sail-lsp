use chumsky::{
    input::{Input as _, SpannedInput},
    prelude::*,
    ParseResult, Parser,
};

use crate::{
    ast::{
        Attribute, AttributeData, AttributeEntry, BitfieldField, BlockItem, Call, CallableClause,
        CallableDef, CallableDefKind, CallableQuantifier, CallableSpec, CallableSpecKind,
        ConstraintDef, DefModifiers, DefaultDef, DirectiveDef, EndDef, EnumFunction, EnumMember,
        Expr, ExternBinding, ExternPurity, ExternSpec, FieldExpr, FieldPattern, FixityDef,
        FixityKind, ForeachExpr, InstantiationDef, InstantiationSubstitution, LetBinding, Literal,
        LoopMeasure, MappingArm, MappingArmDirection, MappingBody, MatchCase, NamedDef,
        NamedDefDetail, NamedDefKind, Pattern, QuantifierVar, RecMeasure, ScatteredClauseDef,
        ScatteredClauseKind, ScatteredDef, ScatteredKind, SourceFile, Spanned,
        TerminationMeasureDef, TerminationMeasureKind, TopLevelDef, TypeAliasDef, TypeArrowKind,
        TypeExpr, TypeParam, TypeParamSpec, TypeParamTail, TypedField, UnionPayload, UnionVariant,
        VectorUpdate,
    },
    Span, Token,
};

type Input<'a> = SpannedInput<Token, Span, &'a [(Token, Span)]>;
type ParseErr<'a> = extra::Err<Rich<'a, Token, Span>>;

fn bracket_kind(token: &Token) -> Option<(u8, bool)> {
    match token {
        Token::LeftBracket => Some((0, true)),
        Token::RightBracket => Some((0, false)),
        Token::LeftSquareBracket => Some((1, true)),
        Token::RightSquareBracket => Some((1, false)),
        Token::LeftCurlyBracket => Some((2, true)),
        Token::RightCurlyBracket => Some((2, false)),
        Token::LeftCurlyBar => Some((3, true)),
        Token::RightCurlyBar => Some((3, false)),
        Token::LeftSquareBar => Some((4, true)),
        Token::RightSquareBar => Some((4, false)),
        _ => None,
    }
}

fn token_is_open_bracket(token: &Token) -> bool {
    matches!(bracket_kind(token), Some((_, true)))
}

fn token_is_close_bracket(token: &Token) -> bool {
    matches!(bracket_kind(token), Some((_, false)))
}

fn token_is_private_modifier(token: &Token) -> bool {
    match token {
        Token::KwPrivate => true,
        Token::Id(name) => name == "Private" || name == "private",
        _ => false,
    }
}

fn token_starts_declaration(token: &Token) -> bool {
    matches!(
        token,
        Token::KwFunction
            | Token::KwVal
            | Token::KwOutcome
            | Token::KwMapping
            | Token::KwConstraint
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
            | Token::KwDefault
            | Token::KwInstantiation
            | Token::KwInfix
            | Token::KwInfixl
            | Token::KwInfixr
            | Token::KwEnd
            | Token::KwTerminationMeasure
            | Token::Directive { .. }
            | Token::StructuredDirectiveStart(_)
    ) || token_is_private_modifier(token)
}

fn parse_def_modifiers(tokens: &[(Token, Span)], start_idx: usize) -> (DefModifiers, usize) {
    let mut modifiers = DefModifiers::default();
    let mut idx = start_idx;

    loop {
        if matches!(tokens.get(idx), Some((token, _)) if token_is_private_modifier(token)) {
            modifiers.is_private = true;
            idx += 1;
            continue;
        }
        if let Some((attr, next_idx)) = parse_attribute_prefix(tokens, idx) {
            modifiers.attrs.push(attr);
            idx = next_idx;
            continue;
        }
        break;
    }

    (modifiers, idx)
}

fn declaration_starts_at(tokens: &[(Token, Span)], start_idx: usize) -> bool {
    if start_idx >= tokens.len() {
        return false;
    }
    let (_, cursor) = parse_def_modifiers(tokens, start_idx);
    if cursor >= tokens.len() {
        return false;
    }
    if start_idx > 0
        && matches!(tokens[cursor].0, Token::KwRegister)
        && token_can_continue_item(&tokens[start_idx - 1].0)
    {
        return false;
    }
    token_starts_declaration(&tokens[cursor].0)
}

fn apply_modifiers(item: &mut TopLevelDef, modifiers: DefModifiers) {
    match item {
        TopLevelDef::Scattered(def) => def.modifiers = modifiers,
        TopLevelDef::ScatteredClause(def) => def.modifiers = modifiers,
        TopLevelDef::CallableSpec(def) => def.modifiers = modifiers,
        TopLevelDef::CallableDef(def) => def.modifiers = modifiers,
        TopLevelDef::TypeAlias(def) => def.modifiers = modifiers,
        TopLevelDef::Named(def) => def.modifiers = modifiers,
        TopLevelDef::Default(def) => def.modifiers = modifiers,
        TopLevelDef::Fixity(def) => def.modifiers = modifiers,
        TopLevelDef::Instantiation(def) => def.modifiers = modifiers,
        TopLevelDef::Directive(def) => def.modifiers = modifiers,
        TopLevelDef::End(def) => def.modifiers = modifiers,
        TopLevelDef::Constraint(def) => def.modifiers = modifiers,
        TopLevelDef::TerminationMeasure(def) => def.modifiers = modifiers,
    }
}

fn token_starts_body_terminator(token: &Token) -> bool {
    matches!(
        token,
        Token::KwFunction
            | Token::KwVal
            | Token::KwOutcome
            | Token::KwMapping
            | Token::KwConstraint
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
            | Token::KwDefault
            | Token::KwInstantiation
            | Token::KwInfix
            | Token::KwInfixl
            | Token::KwInfixr
            | Token::KwTerminationMeasure
            | Token::KwEnd
            | Token::Directive { .. }
            | Token::StructuredDirectiveStart(_)
    ) || token_is_private_modifier(token)
}

fn token_can_continue_item(token: &Token) -> bool {
    matches!(
        token,
        Token::Colon
            | Token::ColonEqual
            | Token::Comma
            | Token::Dot
            | Token::Equal
            | Token::RightArrow
            | Token::DoubleArrow
            | Token::FatRightArrow
            | Token::LessThan
            | Token::GreaterThan
            | Token::LessThanOrEqualTo
            | Token::GreaterThanOrEqualTo
            | Token::EqualTo
            | Token::NotEqualTo
            | Token::Plus
            | Token::Minus
            | Token::Multiply
            | Token::Divide
            | Token::Modulus
            | Token::And
            | Token::Or
            | Token::Scope
            | Token::Caret
            | Token::At
            | Token::KwAnd
            | Token::KwIn
            | Token::KwWith
    )
}

fn span_for_indices(tokens: &[(Token, Span)], start: usize, end: usize) -> Span {
    Span::new(tokens[start].1.start, tokens[end].1.end)
}

fn eoi_span(tokens: &[(Token, Span)]) -> Span {
    tokens
        .last()
        .map(|(_, span)| Span::new(span.end, span.end))
        .unwrap_or_else(|| Span::new(0, 0))
}

fn find_matching_bracket(tokens: &[(Token, Span)], open_idx: usize) -> Option<usize> {
    let (kind, is_open) = bracket_kind(&tokens.get(open_idx)?.0)?;
    if !is_open {
        return None;
    }

    let mut depth = 1_i32;
    for (idx, (token, _)) in tokens.iter().enumerate().skip(open_idx + 1) {
        if let Some((other_kind, other_is_open)) = bracket_kind(token) {
            if other_kind != kind {
                continue;
            }
            if other_is_open {
                depth += 1;
            } else {
                depth -= 1;
                if depth == 0 {
                    return Some(idx);
                }
            }
        }
    }

    None
}

fn find_matching_round_bracket(tokens: &[(Token, Span)], open_idx: usize) -> Option<usize> {
    if tokens.get(open_idx).map(|(token, _)| token) != Some(&Token::LeftBracket) {
        return None;
    }
    find_matching_bracket(tokens, open_idx)
}

fn find_structured_directive_end(tokens: &[(Token, Span)], start_idx: usize) -> Option<usize> {
    let Token::StructuredDirectiveStart(_) = tokens.get(start_idx)?.0 else {
        return None;
    };

    let mut depth = 1_i32;
    for idx in (start_idx + 1)..tokens.len() {
        match tokens[idx].0 {
            Token::LeftCurlyBracket => depth += 1,
            Token::RightCurlyBracket => {
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

fn find_last_top_level_token<F>(
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
    for idx in (start..=end).rev() {
        let token = &tokens[idx].0;
        if token_is_close_bracket(token) {
            depth += 1;
        } else if token_is_open_bracket(token) {
            depth -= 1;
        }

        if depth == 0 && predicate(token) {
            return Some(idx);
        }
    }

    None
}

fn find_top_level_token_including_opens<F>(
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
        if depth == 0 && predicate(token) {
            return Some(idx);
        }
        if token_is_open_bracket(token) {
            depth += 1;
        } else if token_is_close_bracket(token) {
            depth -= 1;
        }
    }

    None
}

fn find_top_level_double_dot(tokens: &[(Token, Span)], start: usize, end: usize) -> Option<usize> {
    if start >= end {
        return None;
    }

    let mut depth = 0_i32;
    for idx in start..end {
        let token = &tokens[idx].0;
        if depth == 0 && token == &Token::Dot && tokens[idx + 1].0 == Token::Dot {
            return Some(idx);
        }
        if token_is_open_bracket(token) {
            depth += 1;
        } else if token_is_close_bracket(token) {
            depth -= 1;
        }
    }

    None
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
    let mut depth = 0_i32;
    let mut segment_start = start;

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

fn split_switch_cases(tokens: &[(Token, Span)], start: usize, end: usize) -> Vec<(usize, usize)> {
    if start > end {
        return Vec::new();
    }

    let mut segments = Vec::new();
    let mut depth = 0_i32;
    let mut case_start = None;

    for idx in start..=end {
        let token = &tokens[idx].0;
        if token_is_open_bracket(token) {
            depth += 1;
        } else if token_is_close_bracket(token) {
            depth -= 1;
        }

        if depth == 0 && *token == Token::KwCase {
            if let Some(segment_start) = case_start.replace(idx) {
                if segment_start < idx {
                    segments.push((segment_start, idx - 1));
                }
            }
        }
    }

    if let Some(segment_start) = case_start {
        segments.push((segment_start, end));
    } else {
        segments.push((start, end));
    }

    segments
}

fn split_top_level_commas(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> (Vec<(usize, usize)>, Vec<Span>) {
    if start > end {
        return (Vec::new(), Vec::new());
    }

    let mut segments = Vec::new();
    let mut separators = Vec::new();
    let mut depth = 0_i32;
    let mut segment_start = start;

    for idx in start..=end {
        let token = &tokens[idx].0;
        if token_is_open_bracket(token) {
            depth += 1;
        } else if token_is_close_bracket(token) {
            depth -= 1;
        }

        if depth == 0 && *token == Token::Comma {
            if segment_start < idx {
                segments.push((segment_start, idx - 1));
            }
            separators.push(tokens[idx].1);
            segment_start = idx + 1;
        }
    }

    if segment_start <= end {
        segments.push((segment_start, end));
    }

    (segments, separators)
}

fn strip_wrapping_parens(tokens: &[(Token, Span)], start: usize, end: usize) -> (usize, usize) {
    if start < end
        && tokens[start].0 == Token::LeftBracket
        && find_matching_round_bracket(tokens, start) == Some(end)
    {
        (start + 1, end - 1)
    } else {
        (start, end)
    }
}

fn token_as_ident(token: &Token) -> Option<String> {
    match token {
        Token::Id(name) => Some(name.clone()),
        Token::KwInt
        | Token::KwBool
        | Token::KwCase
        | Token::KwTypeUpper
        | Token::KwOrder
        | Token::KwDec
        | Token::KwDownto
        | Token::KwFrom
        | Token::KwInc
        | Token::KwPrivate
        | Token::KwTo
        | Token::KwWhen => Some(token.to_string()),
        _ => None,
    }
}

fn parse_name_with_optional_hash(
    tokens: &[(Token, Span)],
    idx: usize,
) -> Option<(Spanned<String>, usize)> {
    if matches!(tokens.get(idx), Some((Token::Id(keyword), _)) if keyword == "operator") {
        let op_start = idx + 1;
        let op_end = find_top_level_token_including_opens(
            tokens,
            op_start,
            tokens.len().saturating_sub(1),
            |token| {
                matches!(
                    token,
                    Token::LeftBracket | Token::Colon | Token::Equal | Token::RightArrow
                )
            },
        )
        .map_or(tokens.len().saturating_sub(1), |stop_idx| {
            stop_idx.saturating_sub(1)
        });
        if op_start > op_end {
            return None;
        }
        return Some((
            (
                tokens_text(tokens, op_start, op_end),
                span_for_indices(tokens, op_start, op_end),
            ),
            op_end + 1,
        ));
    }

    let mut name = token_as_ident(&tokens.get(idx)?.0)?;
    let mut end_idx = idx;
    if matches!(tokens.get(idx + 1), Some((Token::Hash, _))) {
        name.push('#');
        end_idx = idx + 1;
    }
    Some(((name, span_for_indices(tokens, idx, end_idx)), end_idx + 1))
}

fn token_is_ident_name(token: &Token, expected: &str) -> bool {
    matches!(token, Token::Id(name) if name == expected)
}

fn token_as_literal(token: &Token) -> Option<Literal> {
    match token {
        Token::KwTrue => Some(Literal::Bool(true)),
        Token::KwFalse => Some(Literal::Bool(false)),
        Token::Unit => Some(Literal::Unit),
        Token::Num(text) | Token::Real(text) => Some(Literal::Number(text.clone())),
        Token::Bin(text) => Some(Literal::Binary(text.clone())),
        Token::Hex(text) => Some(Literal::Hex(text.clone())),
        Token::String(text) => Some(Literal::String(text.clone())),
        Token::KwUndefined | Token::KwUndef => Some(Literal::Undefined),
        Token::KwBitzero => Some(Literal::BitZero),
        Token::KwBitone => Some(Literal::BitOne),
        _ => None,
    }
}

fn kind_from_token(token: &Token) -> Option<String> {
    match token {
        Token::KwInt | Token::KwBool | Token::KwTypeUpper | Token::KwOrder => {
            Some(token.to_string())
        }
        Token::Id(name) if name == "Nat" => Some(name.clone()),
        _ => None,
    }
}

fn kind_span(tokens: &[(Token, Span)], start: usize, end: usize) -> Option<Spanned<String>> {
    (start == end)
        .then(|| kind_from_token(&tokens[start].0).map(|kind| (kind, tokens[start].1)))
        .flatten()
}

fn parse_type_param_group(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Option<Vec<TypeParam>> {
    if start > end {
        return None;
    }

    if start == end {
        let Token::TyVal(name) = &tokens[start].0 else {
            return None;
        };
        return Some(vec![TypeParam {
            name: (name.clone(), tokens[start].1),
            kind: None,
            is_constant: false,
        }]);
    }

    if tokens[start].0 != Token::LeftBracket
        || find_matching_round_bracket(tokens, start) != Some(end)
    {
        return None;
    }

    let mut cursor = start + 1;
    let mut is_constant = false;
    if matches!(tokens.get(cursor), Some((Token::KwConstant, _))) {
        is_constant = true;
        cursor += 1;
    }
    let mut names = Vec::new();
    while cursor < end {
        let Token::TyVal(name) = &tokens.get(cursor)?.0 else {
            break;
        };
        names.push((name.clone(), tokens[cursor].1));
        cursor += 1;
    }
    if names.is_empty() {
        return None;
    }

    let kind = if cursor < end && tokens[cursor].0 == Token::Colon {
        kind_span(tokens, cursor + 1, end.saturating_sub(1))
    } else if cursor == end {
        None
    } else {
        return None;
    };

    Some(
        names
            .into_iter()
            .map(|(name, span)| TypeParam {
                name: (name, span),
                kind: kind.clone(),
                is_constant,
            })
            .collect(),
    )
}

fn parse_type_param_spec(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Option<Spanned<TypeParamSpec>> {
    if start > end || tokens.get(start)?.0 != Token::LeftBracket {
        return None;
    }

    let close_idx = find_matching_round_bracket(tokens, start)?;
    if close_idx > end {
        return None;
    }

    let params = if start + 1 < close_idx {
        split_top_level_segments(tokens, start + 1, close_idx - 1, |token| {
            *token == Token::Comma
        })
        .into_iter()
        .map(|(segment_start, segment_end)| {
            parse_type_param_group(tokens, segment_start, segment_end)
        })
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect()
    } else {
        Vec::new()
    };

    let tail = if close_idx == end {
        None
    } else {
        match tokens.get(close_idx + 1)?.0 {
            Token::Comma if close_idx + 2 <= end => Some(TypeParamTail::Type(parse_type_expr(
                tokens,
                close_idx + 2,
                end,
            ))),
            Token::KwConstraint if close_idx + 2 <= end => Some(TypeParamTail::Constraint(
                parse_type_expr(tokens, close_idx + 2, end),
            )),
            _ => None,
        }
    };

    Some((
        TypeParamSpec { params, tail },
        span_for_indices(tokens, start, end),
    ))
}

fn is_assignment_token(token: &Token) -> bool {
    matches!(token, Token::Equal | Token::ColonEqual)
}

fn find_top_level_assignment_token(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Option<usize> {
    find_top_level_token(tokens, start, end, is_assignment_token)
}

fn find_matching_else_token(tokens: &[(Token, Span)], start: usize, end: usize) -> Option<usize> {
    if start > end {
        return None;
    }

    let mut depth = 0_i32;
    let mut pending_nested_ifs = 0_i32;
    for idx in start..=end {
        let token = &tokens[idx].0;
        if token_is_open_bracket(token) {
            depth += 1;
            continue;
        }
        if token_is_close_bracket(token) {
            depth -= 1;
            continue;
        }
        if depth != 0 {
            continue;
        }
        match token {
            Token::KwIf => pending_nested_ifs += 1,
            Token::KwElse if pending_nested_ifs == 0 => return Some(idx),
            Token::KwElse => pending_nested_ifs -= 1,
            _ => {}
        }
    }

    None
}

fn prefix_op(tokens: &[(Token, Span)], idx: usize, end: usize) -> Option<(String, Span, usize)> {
    if idx > end {
        return None;
    }
    match &tokens[idx].0 {
        Token::Minus | Token::Multiply => Some((tokens[idx].0.to_string(), tokens[idx].1, idx + 1)),
        Token::Id(name) if name == "~" => Some((name.clone(), tokens[idx].1, idx + 1)),
        Token::Num(value) if value == "2" && idx < end && tokens[idx + 1].0 == Token::Caret => {
            Some((
                "2^".to_string(),
                span_for_indices(tokens, idx, idx + 1),
                idx + 2,
            ))
        }
        _ => None,
    }
}

fn match_composite_infix_op(
    tokens: &[(Token, Span)],
    idx: usize,
    end: usize,
) -> Option<(String, Span, usize)> {
    if idx > end {
        return None;
    }

    if idx + 2 <= end {
        match (&tokens[idx].0, &tokens[idx + 1].0, &tokens[idx + 2].0) {
            (Token::LessThan, Token::LessThan, Token::LessThan) => {
                return Some((
                    "<<<".to_string(),
                    span_for_indices(tokens, idx, idx + 2),
                    idx + 3,
                ));
            }
            (Token::GreaterThan, Token::GreaterThan, Token::GreaterThan) => {
                return Some((
                    ">>>".to_string(),
                    span_for_indices(tokens, idx, idx + 2),
                    idx + 3,
                ));
            }
            _ => {}
        }
    }

    if idx < end {
        match (&tokens[idx].0, &tokens[idx + 1].0) {
            (Token::EqualTo, Token::GreaterThan) => {
                return Some((
                    "==>".to_string(),
                    span_for_indices(tokens, idx, idx + 1),
                    idx + 2,
                ));
            }
            (Token::LessThanOrEqualTo, Token::Id(suffix))
            | (Token::GreaterThanOrEqualTo, Token::Id(suffix))
                if suffix == "_u" || suffix == "_s" =>
            {
                return Some((
                    format!("{}{}", tokens[idx].0, suffix),
                    span_for_indices(tokens, idx, idx + 1),
                    idx + 2,
                ));
            }
            (Token::LessThan, Token::Id(suffix)) | (Token::GreaterThan, Token::Id(suffix))
                if suffix == "_u" || suffix == "_s" =>
            {
                return Some((
                    format!("{}{}", tokens[idx].0, suffix),
                    span_for_indices(tokens, idx, idx + 1),
                    idx + 2,
                ));
            }
            _ => {}
        }
    }

    None
}

fn is_type_op(token: &Token) -> bool {
    matches!(
        token,
        Token::EqualTo
            | Token::NotEqualTo
            | Token::LessThan
            | Token::GreaterThan
            | Token::LessThanOrEqualTo
            | Token::GreaterThanOrEqualTo
            | Token::Minus
            | Token::Or
            | Token::At
            | Token::Scope
            | Token::Caret
            | Token::Multiply
            | Token::Plus
            | Token::Divide
            | Token::Modulus
            | Token::And
            | Token::KwAnd
            | Token::KwIn
    ) || matches!(token, Token::Id(name) if name == "or" || name == "xor")
}

fn is_exp_op(token: &Token) -> bool {
    matches!(
        token,
        Token::EqualTo
            | Token::NotEqualTo
            | Token::LessThan
            | Token::GreaterThan
            | Token::LessThanOrEqualTo
            | Token::GreaterThanOrEqualTo
            | Token::Minus
            | Token::Or
            | Token::At
            | Token::Scope
            | Token::Caret
            | Token::Multiply
            | Token::Plus
            | Token::Divide
            | Token::Modulus
            | Token::And
            | Token::KwAnd
    ) || matches!(token, Token::Id(name) if name == "or" || name == "xor")
}

fn is_pat_op(token: &Token) -> bool {
    matches!(token, Token::At | Token::Scope | Token::Caret)
}

fn error_text(tokens: &[(Token, Span)], start: usize, end: usize) -> String {
    if start > end || start >= tokens.len() || end >= tokens.len() {
        String::new()
    } else {
        tokens_text(tokens, start, end)
    }
}

fn error_expr(tokens: &[(Token, Span)], start: usize, end: usize) -> Spanned<Expr> {
    (
        Expr::Error(error_text(tokens, start, end)),
        if start <= end && end < tokens.len() {
            span_for_indices(tokens, start, end)
        } else {
            fallback_span(tokens, start)
        },
    )
}

fn error_pattern(tokens: &[(Token, Span)], start: usize, end: usize) -> Spanned<Pattern> {
    (
        Pattern::Error(error_text(tokens, start, end)),
        if start <= end && end < tokens.len() {
            span_for_indices(tokens, start, end)
        } else {
            fallback_span(tokens, start)
        },
    )
}

fn pattern_if_structured(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Option<Spanned<Pattern>> {
    let pattern = parse_pattern(tokens, start, end);
    (!matches!(pattern.0, Pattern::Error(_))).then_some(pattern)
}

fn fallback_span(tokens: &[(Token, Span)], idx: usize) -> Span {
    tokens
        .get(idx)
        .map(|(_, span)| *span)
        .or_else(|| tokens.last().map(|(_, span)| Span::new(span.end, span.end)))
        .unwrap_or_else(|| Span::new(0, 0))
}

fn tokens_text(tokens: &[(Token, Span)], start: usize, end: usize) -> String {
    if start > end {
        return String::new();
    }
    let mut out = String::new();
    for idx in start..=end {
        out.push_str(&tokens[idx].0.to_string());
    }
    out
}

fn parse_attribute_data(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Option<Spanned<AttributeData>> {
    if start > end {
        return None;
    }

    let span = span_for_indices(tokens, start, end);
    match &tokens[start].0 {
        Token::LeftCurlyBracket if find_matching_bracket(tokens, start) == Some(end) => {
            let entries = if start + 1 < end {
                split_top_level_segments(tokens, start + 1, end - 1, |token| *token == Token::Comma)
                    .into_iter()
                    .map(|(segment_start, segment_end)| {
                        parse_attribute_entry(tokens, segment_start, segment_end)
                    })
                    .collect::<Option<Vec<_>>>()?
            } else {
                Vec::new()
            };
            Some((AttributeData::Object(entries), span))
        }
        Token::LeftSquareBracket if find_matching_bracket(tokens, start) == Some(end) => {
            let items = if start + 1 < end {
                split_top_level_segments(tokens, start + 1, end - 1, |token| *token == Token::Comma)
                    .into_iter()
                    .map(|(segment_start, segment_end)| {
                        parse_attribute_data(tokens, segment_start, segment_end)
                    })
                    .collect::<Option<Vec<_>>>()?
            } else {
                Vec::new()
            };
            Some((AttributeData::Array(items), span))
        }
        Token::KwTrue if start == end => Some((AttributeData::Bool(true), span)),
        Token::KwFalse if start == end => Some((AttributeData::Bool(false), span)),
        Token::String(text) if start == end => Some((AttributeData::String(text.clone()), span)),
        Token::Num(text) | Token::Real(text) | Token::Bin(text) | Token::Hex(text)
            if start == end =>
        {
            Some((AttributeData::Number(text.clone()), span))
        }
        _ if start == end => {
            token_as_ident(&tokens[start].0).map(|ident| (AttributeData::Ident(ident), span))
        }
        _ => None,
    }
}

fn parse_attribute_entry(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Option<Spanned<AttributeEntry>> {
    if start > end {
        return None;
    }
    let eq_idx = find_top_level_token(tokens, start, end, |token| *token == Token::Equal)?;
    if eq_idx <= start || eq_idx >= end {
        return None;
    }
    let key = token_as_ident(&tokens[start].0)?;
    Some((
        AttributeEntry {
            key: (key, tokens[start].1),
            value: parse_attribute_data(tokens, eq_idx + 1, end)?,
        },
        span_for_indices(tokens, start, end),
    ))
}

fn parse_attribute_prefix(
    tokens: &[(Token, Span)],
    start_idx: usize,
) -> Option<(Spanned<Attribute>, usize)> {
    if tokens.get(start_idx)?.0 != Token::Dollar
        || tokens.get(start_idx + 1)?.0 != Token::LeftSquareBracket
    {
        return None;
    }
    let close_idx = find_matching_bracket(tokens, start_idx + 1)?;
    if close_idx <= start_idx + 2 {
        return None;
    }
    let name_idx = start_idx + 2;
    let name = token_as_ident(&tokens[name_idx].0)?;
    let data = (name_idx < close_idx.saturating_sub(1)).then(|| {
        (
            tokens_text(tokens, name_idx + 1, close_idx - 1),
            span_for_indices(tokens, name_idx + 1, close_idx - 1),
        )
    });
    let parsed_data = if name_idx < close_idx.saturating_sub(1) {
        parse_attribute_data(tokens, name_idx + 1, close_idx - 1)
    } else {
        None
    };
    Some((
        (
            Attribute {
                name: (name, tokens[name_idx].1),
                data,
                parsed_data,
            },
            span_for_indices(tokens, start_idx, close_idx),
        ),
        close_idx + 1,
    ))
}

fn parse_extern_binding(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Option<Spanned<ExternBinding>> {
    let colon_idx = find_top_level_token(tokens, start, end, |token| *token == Token::Colon)?;
    if colon_idx >= end {
        return None;
    }
    let name = match &tokens[start].0 {
        Token::Underscore => None,
        _ => token_as_ident(&tokens[start].0).map(|name| (name, tokens[start].1)),
    };
    Some((
        ExternBinding {
            name,
            value: (
                tokens_text(tokens, colon_idx + 1, end),
                span_for_indices(tokens, colon_idx + 1, end),
            ),
        },
        span_for_indices(tokens, start, end),
    ))
}

fn parse_effect_name(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Option<Spanned<String>> {
    if start > end {
        return None;
    }
    Some((
        tokens_text(tokens, start, end),
        span_for_indices(tokens, start, end),
    ))
}

fn parse_extern_spec(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Option<Spanned<ExternSpec>> {
    if start > end {
        return None;
    }

    let (purity, body_start) = match tokens[start].0 {
        Token::KwPure => (Some(ExternPurity::Pure), start + 1),
        Token::Id(ref name) if name == "impure" => (Some(ExternPurity::Impure), start + 1),
        _ => (None, start),
    };
    if body_start > end {
        return None;
    }

    if let Token::String(text) = &tokens[body_start].0 {
        if body_start == end {
            return Some((
                ExternSpec::String {
                    purity,
                    value: (text.clone(), tokens[body_start].1),
                },
                span_for_indices(tokens, start, end),
            ));
        }
    }

    if tokens[body_start].0 == Token::LeftCurlyBracket
        && find_matching_bracket(tokens, body_start) == Some(end)
    {
        let bindings = if body_start + 1 < end {
            split_top_level_segments(tokens, body_start + 1, end - 1, |token| {
                *token == Token::Comma
            })
            .into_iter()
            .filter_map(|(segment_start, segment_end)| {
                parse_extern_binding(tokens, segment_start, segment_end)
            })
            .collect()
        } else {
            Vec::new()
        };
        return Some((
            ExternSpec::Bindings { purity, bindings },
            span_for_indices(tokens, start, end),
        ));
    }

    None
}

fn parse_config_path(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Option<Vec<Spanned<String>>> {
    let Token::Id(first) = &tokens.get(start)?.0 else {
        return None;
    };
    if first != "config" {
        return None;
    }
    let mut parts = Vec::new();
    let mut idx = start + 1;
    while idx <= end {
        let Some(part) = token_as_ident(&tokens[idx].0) else {
            break;
        };
        parts.push((part, tokens[idx].1));
        idx += 1;
        if idx <= end && tokens[idx].0 == Token::Dot {
            idx += 1;
        } else {
            break;
        }
    }
    (idx == end + 1 && !parts.is_empty()).then_some(parts)
}

fn parse_named_member_list(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
    kind: NamedDefKind,
) -> Vec<Spanned<String>> {
    if start > end {
        return Vec::new();
    }

    if tokens[start].0 == Token::LeftCurlyBracket
        && find_matching_bracket(tokens, start) == Some(end)
    {
        return split_top_level_segments(tokens, start + 1, end.saturating_sub(1), |token| {
            *token == Token::Comma
        })
        .into_iter()
        .filter_map(|(segment_start, segment_end)| match kind {
            NamedDefKind::Enum => {
                let name_end = find_top_level_token(tokens, segment_start, segment_end, |token| {
                    *token == Token::FatRightArrow
                })
                .map_or(segment_end, |idx| idx.saturating_sub(1));
                if matches!(&tokens[segment_start].0, Token::Id(_)) && segment_start == name_end {
                    token_as_ident(&tokens[segment_start].0)
                        .map(|name| (name, tokens[segment_start].1))
                } else {
                    None
                }
            }
            NamedDefKind::Union => {
                let colon_idx =
                    find_top_level_token(tokens, segment_start, segment_end, |token| {
                        *token == Token::Colon
                    })?;
                (segment_start..colon_idx).find_map(|idx| {
                    token_as_ident(&tokens[idx].0).map(|name| (name, tokens[idx].1))
                })
            }
            _ => {
                if matches!(&tokens[segment_start].0, Token::Id(_)) && segment_start == segment_end
                {
                    token_as_ident(&tokens[segment_start].0)
                        .map(|name| (name, tokens[segment_start].1))
                } else {
                    None
                }
            }
        })
        .collect();
    }

    if matches!(kind, NamedDefKind::Enum | NamedDefKind::Overload) {
        return split_top_level_segments(tokens, start, end, |token| *token == Token::Or)
            .into_iter()
            .filter_map(|(segment_start, segment_end)| {
                if segment_start == segment_end {
                    token_as_ident(&tokens[segment_start].0)
                        .map(|name| (name, tokens[segment_start].1))
                } else {
                    None
                }
            })
            .collect();
    }

    Vec::new()
}

fn parse_enum_member(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Option<Spanned<EnumMember>> {
    if start > end {
        return None;
    }
    let name = token_as_ident(&tokens[start].0)?;
    let span = span_for_indices(tokens, start, end);
    let value = find_top_level_token(tokens, start, end, |token| *token == Token::FatRightArrow)
        .filter(|arrow_idx| *arrow_idx > start && *arrow_idx < end)
        .map(|arrow_idx| parse_expr(tokens, arrow_idx + 1, end));
    Some((
        EnumMember {
            name: (name, tokens[start].1),
            value,
        },
        span,
    ))
}

fn parse_enum_member_list(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Vec<Spanned<EnumMember>> {
    if start > end {
        return Vec::new();
    }

    if tokens[start].0 == Token::LeftCurlyBracket
        && find_matching_bracket(tokens, start) == Some(end)
    {
        return split_top_level_segments(tokens, start + 1, end.saturating_sub(1), |token| {
            *token == Token::Comma
        })
        .into_iter()
        .filter_map(|(segment_start, segment_end)| {
            parse_enum_member(tokens, segment_start, segment_end)
        })
        .collect();
    }

    split_top_level_segments(tokens, start, end, |token| *token == Token::Or)
        .into_iter()
        .filter_map(|(segment_start, segment_end)| {
            parse_enum_member(tokens, segment_start, segment_end)
        })
        .collect()
}

fn parse_enum_function(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Option<Spanned<EnumFunction>> {
    if start >= end {
        return None;
    }
    let arrow_idx = find_top_level_token(tokens, start, end, |token| *token == Token::RightArrow)?;
    if arrow_idx <= start || arrow_idx >= end {
        return None;
    }
    let name = token_as_ident(&tokens[start].0)?;
    Some((
        EnumFunction {
            name: (name, tokens[start].1),
            ty: parse_type_expr(tokens, arrow_idx + 1, end),
        },
        span_for_indices(tokens, start, end),
    ))
}

fn parse_enum_function_list(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Vec<Spanned<EnumFunction>> {
    if start > end {
        return Vec::new();
    }

    split_top_level_segments(tokens, start, end, |token| *token == Token::Comma)
        .into_iter()
        .filter_map(|(segment_start, segment_end)| {
            parse_enum_function(tokens, segment_start, segment_end)
        })
        .collect()
}

fn parse_typed_field(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Option<Spanned<TypedField>> {
    let colon_idx = find_top_level_token(tokens, start, end, |token| *token == Token::Colon)?;
    if colon_idx <= start || colon_idx >= end {
        return None;
    }
    let name = token_as_ident(&tokens[start].0)?;
    Some((
        TypedField {
            name: (name, tokens[start].1),
            ty: parse_type_expr(tokens, colon_idx + 1, end),
        },
        span_for_indices(tokens, start, end),
    ))
}

fn parse_typed_field_block(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Vec<Spanned<TypedField>> {
    if start >= end
        || tokens[start].0 != Token::LeftCurlyBracket
        || find_matching_bracket(tokens, start) != Some(end)
    {
        return Vec::new();
    }

    split_top_level_segments(tokens, start + 1, end - 1, |token| *token == Token::Comma)
        .into_iter()
        .filter_map(|(segment_start, segment_end)| {
            parse_typed_field(tokens, segment_start, segment_end)
        })
        .collect()
}

fn parse_union_variant(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Option<Spanned<UnionVariant>> {
    let span = span_for_indices(tokens, start, end);
    let colon_idx = find_top_level_token(tokens, start, end, |token| *token == Token::Colon)?;
    if colon_idx <= start || colon_idx >= end {
        return None;
    }
    let name = token_as_ident(&tokens[start].0)?;
    let payload = if tokens[colon_idx + 1].0 == Token::LeftCurlyBracket
        && find_matching_bracket(tokens, colon_idx + 1) == Some(end)
    {
        UnionPayload::Struct {
            fields: parse_typed_field_block(tokens, colon_idx + 1, end),
        }
    } else {
        UnionPayload::Type(parse_type_expr(tokens, colon_idx + 1, end))
    };
    Some((
        UnionVariant {
            name: (name, tokens[start].1),
            payload,
        },
        span,
    ))
}

fn parse_union_variant_block(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Vec<Spanned<UnionVariant>> {
    if start >= end
        || tokens[start].0 != Token::LeftCurlyBracket
        || find_matching_bracket(tokens, start) != Some(end)
    {
        return Vec::new();
    }

    let inner_start = start + 1;
    let inner_end = end - 1;
    let mut segments = Vec::new();
    let mut depth = 0_i32;
    let mut segment_start = inner_start;
    let mut idx = inner_start;

    while idx <= inner_end {
        let token = &tokens[idx].0;
        if token_is_open_bracket(token) {
            depth += 1;
        } else if token_is_close_bracket(token) {
            depth -= 1;
        }

        let next_starts_variant = depth == 0
            && idx > segment_start
            && matches!(tokens[idx].0, Token::Id(_))
            && matches!(tokens.get(idx + 1), Some((Token::Colon, _)));

        if depth == 0 && token == &Token::Comma {
            if segment_start < idx {
                segments.push((segment_start, idx - 1));
            }
            segment_start = idx + 1;
        } else if next_starts_variant {
            segments.push((segment_start, idx - 1));
            segment_start = idx;
        }

        idx += 1;
    }

    if segment_start <= inner_end {
        segments.push((segment_start, inner_end));
    }

    segments
        .into_iter()
        .filter_map(|(segment_start, segment_end)| {
            parse_union_variant(tokens, segment_start, segment_end)
        })
        .collect()
}

fn parse_bitfield_field(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Option<Spanned<BitfieldField>> {
    let span = span_for_indices(tokens, start, end);
    let colon_idx = find_top_level_token(tokens, start, end, |token| *token == Token::Colon)?;
    if colon_idx <= start || colon_idx >= end {
        return None;
    }
    let name = token_as_ident(&tokens[start].0)?;
    let (high, low) = if let Some(dd_idx) = find_top_level_double_dot(tokens, colon_idx + 1, end) {
        (
            parse_type_expr(tokens, colon_idx + 1, dd_idx.saturating_sub(1)),
            Some(parse_type_expr(tokens, dd_idx + 2, end)),
        )
    } else {
        (parse_type_expr(tokens, colon_idx + 1, end), None)
    };
    Some((
        BitfieldField {
            name: (name, tokens[start].1),
            high,
            low,
        },
        span,
    ))
}

fn parse_bitfield_field_block(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Vec<Spanned<BitfieldField>> {
    if start >= end
        || tokens[start].0 != Token::LeftCurlyBracket
        || find_matching_bracket(tokens, start) != Some(end)
    {
        return Vec::new();
    }

    split_top_level_segments(tokens, start + 1, end - 1, |token| *token == Token::Comma)
        .into_iter()
        .filter_map(|(segment_start, segment_end)| {
            parse_bitfield_field(tokens, segment_start, segment_end)
        })
        .collect()
}

fn parse_type_expr(tokens: &[(Token, Span)], start: usize, end: usize) -> Spanned<TypeExpr> {
    if start > end {
        return (TypeExpr::Error(String::new()), fallback_span(tokens, start));
    }
    let span = span_for_indices(tokens, start, end);

    if tokens[start].0 == Token::LeftBracket
        && find_matching_round_bracket(tokens, start) == Some(end)
    {
        if start + 1 == end {
            return (TypeExpr::Tuple(Vec::new()), span);
        }
        let inner =
            split_top_level_segments(tokens, start + 1, end - 1, |token| *token == Token::Comma);
        if inner.len() > 1 {
            return (
                TypeExpr::Tuple(
                    inner
                        .into_iter()
                        .map(|(segment_start, segment_end)| {
                            parse_type_expr(tokens, segment_start, segment_end)
                        })
                        .collect(),
                ),
                span,
            );
        }
        return parse_type_expr(tokens, start + 1, end - 1);
    }

    let (start, end) = strip_wrapping_parens(tokens, start, end);

    if start > end {
        return (TypeExpr::Error(String::new()), span);
    }

    if tokens[start].0 == Token::KwForall {
        if let Some(dot_idx) =
            find_top_level_token(tokens, start + 1, end, |token| *token == Token::Dot)
        {
            let mut vars = Vec::new();
            let mut constraint_start = None;
            let mut cursor = start + 1;
            while cursor < dot_idx {
                if tokens[cursor].0 == Token::Comma {
                    cursor += 1;
                    continue;
                }

                let group_end = if tokens[cursor].0 == Token::LeftBracket {
                    let Some(close_idx) = find_matching_round_bracket(tokens, cursor) else {
                        constraint_start = Some(cursor);
                        break;
                    };
                    if close_idx >= dot_idx {
                        constraint_start = Some(cursor);
                        break;
                    }
                    close_idx
                } else {
                    cursor
                };

                if let Some(group) = parse_type_param_group(tokens, cursor, group_end) {
                    let after_group = group_end + 1;
                    if after_group < dot_idx
                        && tokens[after_group].0 != Token::Comma
                        && !matches!(tokens[after_group].0, Token::TyVal(_) | Token::LeftBracket)
                    {
                        constraint_start = Some(cursor);
                        break;
                    }
                    vars.extend(group);
                    cursor = after_group;
                    continue;
                }

                constraint_start = Some(cursor);
                break;
            }

            if !vars.is_empty() && dot_idx < end {
                let constraint = constraint_start
                    .filter(|constraint_start| *constraint_start < dot_idx)
                    .map(|constraint_start| {
                        Box::new(parse_type_expr(
                            tokens,
                            constraint_start,
                            dot_idx.saturating_sub(1),
                        ))
                    });
                return (
                    TypeExpr::Forall {
                        vars,
                        constraint,
                        body: Box::new(parse_type_expr(tokens, dot_idx + 1, end)),
                    },
                    span,
                );
            }
        }
    }

    if tokens[start].0 == Token::LeftCurlyBracket
        && find_matching_bracket(tokens, start) == Some(end)
    {
        if let Some(dot_idx) =
            find_top_level_token(tokens, start + 1, end.saturating_sub(1), |token| {
                *token == Token::Dot
            })
        {
            let mut binders = Vec::new();
            let mut cursor = start + 1;
            let mut constraint_start = None;

            while cursor < dot_idx {
                if tokens[cursor].0 == Token::Comma {
                    cursor += 1;
                    continue;
                }

                let group_end = if tokens[cursor].0 == Token::LeftBracket {
                    let Some(close_idx) = find_matching_round_bracket(tokens, cursor) else {
                        constraint_start = Some(cursor);
                        break;
                    };
                    if close_idx >= dot_idx {
                        constraint_start = Some(cursor);
                        break;
                    }
                    close_idx
                } else {
                    cursor
                };

                if let Some(group) = parse_type_param_group(tokens, cursor, group_end) {
                    let after_group = group_end + 1;
                    if after_group < dot_idx
                        && tokens[after_group].0 != Token::Comma
                        && !matches!(tokens[after_group].0, Token::TyVal(_) | Token::LeftBracket)
                    {
                        constraint_start = Some(cursor);
                        break;
                    }
                    binders.extend(group);
                    cursor = after_group;
                    continue;
                }

                constraint_start = Some(cursor);
                break;
            }

            if !binders.is_empty() && dot_idx < end {
                let mut constraint = constraint_start
                    .filter(|constraint_start| *constraint_start < dot_idx)
                    .map(|constraint_start| {
                        Box::new(parse_type_expr(
                            tokens,
                            constraint_start,
                            dot_idx.saturating_sub(1),
                        ))
                    });
                let mut body = parse_type_expr(tokens, dot_idx + 1, end - 1);
                for binder in binders.into_iter().rev() {
                    body = (
                        TypeExpr::Existential {
                            binder,
                            constraint: constraint.take(),
                            body: Box::new(body),
                        },
                        span,
                    );
                }
                return body;
            }
        }
    }

    if let Some(effect_idx) =
        find_top_level_token(tokens, start, end, |token| *token == Token::KwEffect)
    {
        if effect_idx > start
            && effect_idx < end
            && tokens[effect_idx + 1].0 == Token::LeftCurlyBracket
            && find_matching_bracket(tokens, effect_idx + 1) == Some(end)
        {
            let effects = if effect_idx + 2 < end {
                split_top_level_segments(tokens, effect_idx + 2, end - 1, |token| {
                    *token == Token::Comma
                })
                .into_iter()
                .filter_map(|(segment_start, segment_end)| {
                    parse_effect_name(tokens, segment_start, segment_end)
                })
                .collect()
            } else {
                Vec::new()
            };
            return (
                TypeExpr::Effect {
                    ty: Box::new(parse_type_expr(tokens, start, effect_idx.saturating_sub(1))),
                    effects,
                },
                span,
            );
        }
    }

    if tokens[start].0 == Token::KwIf {
        let then_idx =
            find_top_level_token(tokens, start + 1, end, |token| *token == Token::KwThen);
        let else_idx =
            find_top_level_token(tokens, start + 1, end, |token| *token == Token::KwElse);
        if let (Some(then_idx), Some(else_idx)) = (then_idx, else_idx) {
            if start < then_idx && then_idx < else_idx && else_idx < end {
                return (
                    TypeExpr::Conditional {
                        cond: Box::new(parse_type_expr(tokens, start + 1, then_idx - 1)),
                        then_ty: Box::new(parse_type_expr(tokens, then_idx + 1, else_idx - 1)),
                        else_ty: Box::new(parse_type_expr(tokens, else_idx + 1, end)),
                    },
                    span,
                );
            }
        }
    }

    let arrow_segments = split_top_level_segments(tokens, start, end, |token| {
        matches!(token, Token::RightArrow | Token::DoubleArrow)
    });
    if arrow_segments.len() > 1 {
        let arrow_kind = find_top_level_token(tokens, start, end, |token| {
            matches!(token, Token::RightArrow | Token::DoubleArrow)
        })
        .map(|idx| match tokens[idx].0 {
            Token::DoubleArrow => TypeArrowKind::Mapping,
            _ => TypeArrowKind::Function,
        })
        .unwrap_or(TypeArrowKind::Function);

        let mut parsed_segments = arrow_segments
            .into_iter()
            .map(|(segment_start, segment_end)| parse_type_expr(tokens, segment_start, segment_end))
            .collect::<Vec<_>>();
        if let Some(ret) = parsed_segments.pop() {
            return (
                TypeExpr::Arrow {
                    params: parsed_segments,
                    ret: Box::new(ret),
                    kind: arrow_kind,
                },
                span,
            );
        }
    }

    if let Some(parts) = parse_config_path(tokens, start, end) {
        return (TypeExpr::Config(parts), span);
    }

    if let Some((op, op_span, next_idx)) = prefix_op(tokens, start, end) {
        if next_idx <= end {
            return (
                TypeExpr::Prefix {
                    op: (op, op_span),
                    ty: Box::new(parse_type_expr(tokens, next_idx, end)),
                },
                span,
            );
        }
    }

    if tokens[start].0 == Token::KwRegister
        && start < end
        && tokens[start + 1].0 == Token::LeftBracket
        && find_matching_round_bracket(tokens, start + 1) == Some(end)
    {
        return (
            TypeExpr::Register(Box::new(parse_type_expr(
                tokens,
                start + 2,
                end.saturating_sub(1),
            ))),
            span,
        );
    }

    if tokens[start].0 == Token::LeftCurlyBracket
        && find_matching_bracket(tokens, start) == Some(end)
        && find_top_level_token(tokens, start + 1, end.saturating_sub(1), |token| {
            *token == Token::Dot
        })
        .is_none()
    {
        let items = if start + 1 < end {
            split_top_level_segments(tokens, start + 1, end - 1, |token| *token == Token::Comma)
                .into_iter()
                .map(|(segment_start, segment_end)| {
                    parse_type_expr(tokens, segment_start, segment_end)
                })
                .collect()
        } else {
            Vec::new()
        };
        return (TypeExpr::Set(items), span);
    }

    let mut type_segments = Vec::new();
    let mut ops = Vec::new();
    let mut depth = 0_i32;
    let mut segment_start = start;
    let mut idx = start;
    while idx <= end {
        let token = &tokens[idx].0;
        if token_is_open_bracket(token) {
            depth += 1;
        } else if token_is_close_bracket(token) {
            depth -= 1;
        }

        let triple_dot = depth == 0
            && idx > segment_start
            && idx + 2 <= end
            && tokens[idx].0 == Token::Dot
            && tokens[idx + 1].0 == Token::Dot
            && tokens[idx + 2].0 == Token::Dot;
        if triple_dot {
            type_segments.push((segment_start, idx - 1));
            ops.push(("...".to_string(), span_for_indices(tokens, idx, idx + 2)));
            segment_start = idx + 3;
            idx += 3;
            continue;
        }

        if depth == 0 && idx > segment_start && is_type_op(token) {
            type_segments.push((segment_start, idx - 1));
            ops.push((tokens[idx].0.to_string(), tokens[idx].1));
            segment_start = idx + 1;
        }
        idx += 1;
    }
    if !ops.is_empty() {
        type_segments.push((segment_start, end));
        let mut ty = parse_type_expr(tokens, type_segments[0].0, type_segments[0].1);
        for ((op, op_span), (segment_start, segment_end)) in
            ops.into_iter().zip(type_segments.into_iter().skip(1))
        {
            let rhs = parse_type_expr(tokens, segment_start, segment_end);
            ty = (
                TypeExpr::Infix {
                    lhs: Box::new(ty),
                    op: (op, op_span),
                    rhs: Box::new(rhs),
                },
                span,
            );
        }
        return ty;
    }

    if start == end {
        if tokens[start].0 == Token::Underscore {
            return (TypeExpr::Wild, span);
        }
        if tokens[start].0 == Token::KwDec {
            return (TypeExpr::Dec, span);
        }
        if tokens[start].0 == Token::KwInc {
            return (TypeExpr::Inc, span);
        }
        if let Token::TyVal(name) = &tokens[start].0 {
            return (TypeExpr::TypeVar(name.clone()), span);
        }
        if let Some(name) = token_as_ident(&tokens[start].0) {
            return (TypeExpr::Named(name), span);
        }
        if let Some(literal) = token_as_literal(&tokens[start].0) {
            return (
                TypeExpr::Literal(match literal {
                    Literal::Bool(true) => "true".to_string(),
                    Literal::Bool(false) => "false".to_string(),
                    Literal::Unit => "()".to_string(),
                    Literal::Number(text)
                    | Literal::Binary(text)
                    | Literal::Hex(text)
                    | Literal::String(text) => text,
                    Literal::Undefined => "undefined".to_string(),
                    Literal::BitZero => "bitzero".to_string(),
                    Literal::BitOne => "bitone".to_string(),
                }),
                span,
            );
        }
    }

    if let Some(callee) = token_as_ident(&tokens[start].0) {
        if start < end && tokens[start + 1].0 == Token::LeftBracket {
            if let Some(close_idx) = find_matching_round_bracket(tokens, start + 1) {
                if close_idx == end {
                    let args = if start + 2 <= end.saturating_sub(1) {
                        split_top_level_segments(tokens, start + 2, end - 1, |token| {
                            *token == Token::Comma
                        })
                        .into_iter()
                        .map(|(segment_start, segment_end)| {
                            parse_type_expr(tokens, segment_start, segment_end)
                        })
                        .collect()
                    } else {
                        Vec::new()
                    };
                    return (
                        TypeExpr::App {
                            callee: (callee, tokens[start].1),
                            args,
                        },
                        span,
                    );
                }
            }
        }
    }

    (TypeExpr::Error(tokens_text(tokens, start, end)), span)
}

fn parse_field_pattern(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Spanned<FieldPattern> {
    let span = span_for_indices(tokens, start, end);
    if start == end {
        return match &tokens[start].0 {
            Token::Underscore => (FieldPattern::Wild(span), span),
            Token::Id(name) => (
                FieldPattern::Shorthand((name.clone(), tokens[start].1)),
                span,
            ),
            _ => (FieldPattern::Wild(span), span),
        };
    }

    if let Some(eq_idx) = find_top_level_token(tokens, start, end, |token| *token == Token::Equal) {
        if let Token::Id(name) = &tokens[start].0 {
            return (
                FieldPattern::Binding {
                    name: (name.clone(), tokens[start].1),
                    pattern: parse_pattern(tokens, eq_idx + 1, end),
                },
                span,
            );
        }
    }

    match &tokens[start].0 {
        Token::Id(name) => (
            FieldPattern::Shorthand((name.clone(), tokens[start].1)),
            span,
        ),
        _ => (FieldPattern::Wild(span), span),
    }
}

fn parse_pattern(tokens: &[(Token, Span)], start: usize, end: usize) -> Spanned<Pattern> {
    if start > end {
        return (Pattern::Error(String::new()), fallback_span(tokens, start));
    }
    let span = span_for_indices(tokens, start, end);

    if let Some((attr, next_idx)) = parse_attribute_prefix(tokens, start) {
        if next_idx <= end {
            return (
                Pattern::Attribute {
                    attr,
                    pattern: Box::new(parse_pattern(tokens, next_idx, end)),
                },
                span,
            );
        }
    }

    if tokens[start].0 == Token::LeftBracket
        && find_matching_round_bracket(tokens, start) == Some(end)
    {
        if start + 1 == end {
            return (Pattern::Literal(Literal::Unit), span);
        }
        let inner =
            split_top_level_segments(tokens, start + 1, end - 1, |token| *token == Token::Comma);
        if inner.len() > 1 {
            return (
                Pattern::Tuple(
                    inner
                        .into_iter()
                        .map(|(segment_start, segment_end)| {
                            parse_pattern(tokens, segment_start, segment_end)
                        })
                        .collect(),
                ),
                span,
            );
        }
    }

    let (start, end) = strip_wrapping_parens(tokens, start, end);
    if start > end {
        return (Pattern::Error(String::new()), span);
    }

    // Upstream Sail allows both `pattern as Type` and `pattern match Type` (P_var).
    if let Some(as_idx) = find_top_level_token(tokens, start, end, |token| {
        *token == Token::KwAs || *token == Token::KwMatch
    }) {
        if as_idx < end {
            if as_idx + 1 == end {
                if let Some(binding) = token_as_ident(&tokens[end].0) {
                    return (
                        Pattern::AsBinding {
                            pattern: Box::new(parse_pattern(
                                tokens,
                                start,
                                as_idx.saturating_sub(1),
                            )),
                            binding: (binding, tokens[end].1),
                        },
                        span,
                    );
                }
            }
            return (
                Pattern::AsType(
                    Box::new(parse_pattern(tokens, start, as_idx.saturating_sub(1))),
                    parse_type_expr(tokens, as_idx + 1, end),
                ),
                span,
            );
        }
    }

    let pat_segments = split_top_level_segments(tokens, start, end, is_pat_op);
    if pat_segments.len() > 1 {
        let mut op_positions = Vec::new();
        let mut depth = 0_i32;
        for idx in start..=end {
            let token = &tokens[idx].0;
            if token_is_open_bracket(token) {
                depth += 1;
            } else if token_is_close_bracket(token) {
                depth -= 1;
            }
            if depth == 0 && idx > start && is_pat_op(token) {
                op_positions.push(idx);
            }
        }

        let mut pattern = parse_pattern(tokens, pat_segments[0].0, pat_segments[0].1);
        for (op_idx, (segment_start, segment_end)) in op_positions
            .into_iter()
            .zip(pat_segments.into_iter().skip(1))
        {
            let rhs = parse_pattern(tokens, segment_start, segment_end);
            pattern = (
                Pattern::Infix {
                    lhs: Box::new(pattern),
                    op: (tokens[op_idx].0.to_string(), tokens[op_idx].1),
                    rhs: Box::new(rhs),
                },
                span,
            );
        }
        return pattern;
    }

    if let Some(colon_idx) =
        find_top_level_token(tokens, start, end, |token| *token == Token::Colon)
    {
        if colon_idx < end {
            return (
                Pattern::Typed(
                    Box::new(parse_pattern(tokens, start, colon_idx.saturating_sub(1))),
                    parse_type_expr(tokens, colon_idx + 1, end),
                ),
                span,
            );
        }
    }

    if start == end {
        if let Some(literal) = token_as_literal(&tokens[start].0) {
            return (Pattern::Literal(literal), span);
        }
        return match &tokens[start].0 {
            Token::Underscore => (Pattern::Wild, span),
            Token::Id(name) => (Pattern::Ident(name.clone()), span),
            Token::TyVal(name) => (Pattern::TypeVar(name.clone()), span),
            _ => (Pattern::Error(tokens_text(tokens, start, end)), span),
        };
    }

    if tokens[start].0 == Token::LeftSquareBar && find_matching_bracket(tokens, start) == Some(end)
    {
        let items = if start + 1 < end {
            split_top_level_segments(tokens, start + 1, end - 1, |token| *token == Token::Comma)
                .into_iter()
                .map(|(segment_start, segment_end)| {
                    parse_pattern(tokens, segment_start, segment_end)
                })
                .collect()
        } else {
            Vec::new()
        };
        return (Pattern::List(items), span);
    }

    if tokens[start].0 == Token::LeftSquareBracket
        && find_matching_bracket(tokens, start) == Some(end)
    {
        let items = if start + 1 < end {
            split_top_level_segments(tokens, start + 1, end - 1, |token| *token == Token::Comma)
                .into_iter()
                .map(|(segment_start, segment_end)| {
                    parse_pattern(tokens, segment_start, segment_end)
                })
                .collect()
        } else {
            Vec::new()
        };
        return (Pattern::Vector(items), span);
    }

    if tokens[start].0 == Token::KwStruct {
        let name_idx = if start < end && matches!(tokens[start + 1].0, Token::Id(_)) {
            Some(start + 1)
        } else {
            None
        };
        let brace_idx = name_idx.map_or(start + 1, |idx| idx + 1);
        if brace_idx <= end
            && tokens[brace_idx].0 == Token::LeftCurlyBracket
            && find_matching_bracket(tokens, brace_idx) == Some(end)
        {
            let fields = if brace_idx + 1 < end {
                split_top_level_segments(tokens, brace_idx + 1, end - 1, |token| {
                    *token == Token::Comma
                })
                .into_iter()
                .map(|(segment_start, segment_end)| {
                    parse_field_pattern(tokens, segment_start, segment_end)
                })
                .collect()
            } else {
                Vec::new()
            };
            return (
                Pattern::Struct {
                    name: name_idx.and_then(|idx| {
                        token_as_ident(&tokens[idx].0).map(|name| (name, tokens[idx].1))
                    }),
                    fields,
                },
                span,
            );
        }
    }

    if let Some(literal) = token_as_literal(&tokens[start].0) {
        if start == end {
            return (Pattern::Literal(literal), span);
        }
    }

    if let Some(((name, name_span), next_idx)) = parse_name_with_optional_hash(tokens, start) {
        if next_idx <= end && tokens[next_idx].0 == Token::LeftSquareBracket {
            if let Some(close_idx) = find_matching_bracket(tokens, next_idx) {
                if close_idx == end {
                    let inner_start = next_idx + 1;
                    let inner_end = end.saturating_sub(1);
                    if let Some(dd_idx) = find_top_level_double_dot(tokens, inner_start, inner_end)
                    {
                        return (
                            Pattern::RangeIndex {
                                name: (name, name_span),
                                start: parse_type_expr(
                                    tokens,
                                    inner_start,
                                    dd_idx.saturating_sub(1),
                                ),
                                end: parse_type_expr(tokens, dd_idx + 2, inner_end),
                            },
                            span,
                        );
                    }
                    return (
                        Pattern::Index {
                            name: (name, name_span),
                            index: parse_type_expr(tokens, inner_start, inner_end),
                        },
                        span,
                    );
                }
            }
        }
        if next_idx == end && tokens[next_idx].0 == Token::Unit {
            return (
                Pattern::App {
                    callee: (name, name_span),
                    args: vec![unit_pattern(tokens[next_idx].1)],
                },
                span,
            );
        }
        if next_idx <= end && tokens[next_idx].0 == Token::LeftBracket {
            if let Some(close_idx) = find_matching_round_bracket(tokens, next_idx) {
                if close_idx == end {
                    let args = if next_idx + 1 <= end.saturating_sub(1) {
                        split_top_level_segments(tokens, next_idx + 1, end - 1, |token| {
                            *token == Token::Comma
                        })
                        .into_iter()
                        .map(|(segment_start, segment_end)| {
                            parse_pattern(tokens, segment_start, segment_end)
                        })
                        .collect()
                    } else {
                        Vec::new()
                    };
                    return (
                        Pattern::App {
                            callee: (name, name_span),
                            args,
                        },
                        span,
                    );
                }
            }
        }
    }

    error_pattern(tokens, start, end)
}

fn parse_let_binding(tokens: &[(Token, Span)], start: usize, end: usize) -> LetBinding {
    if let Some(eq_idx) = find_top_level_token(tokens, start, end, |token| *token == Token::Equal) {
        return LetBinding {
            pattern: parse_pattern(tokens, start, eq_idx.saturating_sub(1)),
            value: Box::new(parse_expr(tokens, eq_idx + 1, end)),
        };
    }

    LetBinding {
        pattern: parse_pattern(tokens, start, end),
        value: Box::new(error_expr(tokens, start, end)),
    }
}

fn parse_field_expr(tokens: &[(Token, Span)], start: usize, end: usize) -> Spanned<FieldExpr> {
    let span = span_for_indices(tokens, start, end);
    if let Some(eq_idx) = find_top_level_assignment_token(tokens, start, end) {
        return (
            FieldExpr::Assignment {
                target: parse_prefixed_atomic_expr(tokens, start, eq_idx.saturating_sub(1)),
                value: parse_expr(tokens, eq_idx + 1, end),
            },
            span,
        );
    }

    if let Token::Id(name) = &tokens[start].0 {
        return (FieldExpr::Shorthand((name.clone(), tokens[start].1)), span);
    }

    (
        FieldExpr::Shorthand((tokens[start].0.to_string(), tokens[start].1)),
        span,
    )
}

fn parse_vector_update(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Spanned<VectorUpdate> {
    let span = span_for_indices(tokens, start, end);
    if let Some(eq_idx) = find_top_level_assignment_token(tokens, start, end) {
        if let Some(dd_idx) = find_top_level_double_dot(tokens, start, eq_idx.saturating_sub(1)) {
            return (
                VectorUpdate::RangeAssignment {
                    start: parse_prefixed_atomic_expr(tokens, start, dd_idx.saturating_sub(1)),
                    end: parse_prefixed_atomic_expr(tokens, dd_idx + 2, eq_idx.saturating_sub(1)),
                    value: parse_expr(tokens, eq_idx + 1, end),
                },
                span,
            );
        }
        return (
            VectorUpdate::Assignment {
                target: parse_prefixed_atomic_expr(tokens, start, eq_idx.saturating_sub(1)),
                value: parse_expr(tokens, eq_idx + 1, end),
            },
            span,
        );
    }

    if let Token::Id(name) = &tokens[start].0 {
        return (
            VectorUpdate::Shorthand((name.clone(), tokens[start].1)),
            span,
        );
    }

    (
        VectorUpdate::Shorthand((tokens[start].0.to_string(), tokens[start].1)),
        span,
    )
}

fn parse_case(tokens: &[(Token, Span)], start: usize, end: usize) -> Spanned<MatchCase> {
    let span = span_for_indices(tokens, start, end);
    if let Some((attr, next_idx)) = parse_attribute_prefix(tokens, start) {
        if next_idx <= end
            && tokens[next_idx].0 == Token::LeftBracket
            && find_matching_round_bracket(tokens, next_idx) == Some(end)
        {
            let inner_start = next_idx + 1;
            let inner_end = end.saturating_sub(1);
            let mut case = parse_case(tokens, inner_start, inner_end);
            case.0.attrs.insert(0, attr);
            case.1 = span;
            return case;
        }
    }

    let start = if tokens[start].0 == Token::KwCase && start < end {
        start + 1
    } else {
        start
    };
    let arrow_idx = find_top_level_token(tokens, start, end, |token| {
        matches!(token, Token::FatRightArrow | Token::RightArrow)
    });
    let if_idx = find_top_level_token(tokens, start, end, |token| *token == Token::KwIf);

    match (if_idx, arrow_idx) {
        (Some(if_idx), Some(arrow_idx)) if if_idx < arrow_idx && arrow_idx < end => (
            MatchCase {
                attrs: Vec::new(),
                pattern: parse_pattern(tokens, start, if_idx.saturating_sub(1)),
                guard: Some(parse_expr(tokens, if_idx + 1, arrow_idx.saturating_sub(1))),
                body: parse_expr(tokens, arrow_idx + 1, end),
            },
            span,
        ),
        (_, Some(arrow_idx)) if arrow_idx < end => (
            MatchCase {
                attrs: Vec::new(),
                pattern: parse_pattern(tokens, start, arrow_idx.saturating_sub(1)),
                guard: None,
                body: parse_expr(tokens, arrow_idx + 1, end),
            },
            span,
        ),
        _ => (
            MatchCase {
                attrs: Vec::new(),
                pattern: error_pattern(tokens, start, end),
                guard: None,
                body: error_expr(tokens, start, end),
            },
            span,
        ),
    }
}

fn parse_block_item(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
    terminated: bool,
) -> Spanned<BlockItem> {
    let span = span_for_indices(tokens, start, end);
    if tokens[start].0 == Token::KwLet && start < end {
        if !terminated
            && find_top_level_token(tokens, start + 1, end, |token| *token == Token::KwIn).is_some()
        {
            return (BlockItem::Expr(parse_expr(tokens, start, end)), span);
        }
        return (
            BlockItem::Let(parse_let_binding(tokens, start + 1, end)),
            span,
        );
    }

    if tokens[start].0 == Token::KwVar && start < end {
        if !terminated
            && find_top_level_token(tokens, start + 1, end, |token| *token == Token::KwIn).is_some()
        {
            return (BlockItem::Expr(parse_expr(tokens, start, end)), span);
        }
        if let Some(eq_idx) = find_top_level_assignment_token(tokens, start + 1, end) {
            return (
                BlockItem::Var {
                    target: parse_prefixed_atomic_expr(tokens, start + 1, eq_idx.saturating_sub(1)),
                    value: parse_expr(tokens, eq_idx + 1, end),
                },
                span,
            );
        }
    }

    (BlockItem::Expr(parse_expr(tokens, start, end)), span)
}

fn parse_block(tokens: &[(Token, Span)], start: usize, end: usize) -> Spanned<Expr> {
    let span = span_for_indices(tokens, start, end);
    if start + 1 >= end {
        return (Expr::Block(Vec::new()), span);
    }
    let mut items = Vec::new();
    let mut depth = 0_i32;
    let mut segment_start = start + 1;
    for idx in (start + 1)..end {
        let token = &tokens[idx].0;
        if token_is_open_bracket(token) {
            depth += 1;
        } else if token_is_close_bracket(token) {
            depth -= 1;
        }

        if depth == 0 && *token == Token::Semicolon {
            if segment_start < idx {
                items.push(parse_block_item(tokens, segment_start, idx - 1, true));
            }
            segment_start = idx + 1;
        }
    }
    if segment_start < end {
        items.push(parse_block_item(tokens, segment_start, end - 1, false));
    }
    (Expr::Block(items), span)
}

fn parse_update_expr(tokens: &[(Token, Span)], start: usize, end: usize) -> Option<Spanned<Expr>> {
    if tokens[start].0 != Token::LeftCurlyBracket
        || find_matching_bracket(tokens, start) != Some(end)
    {
        return None;
    }
    let with_idx = find_top_level_token(tokens, start + 1, end.saturating_sub(1), |token| {
        *token == Token::KwWith
    })?;
    let fields = if with_idx < end - 1 {
        split_top_level_segments(tokens, with_idx + 1, end - 1, |token| {
            *token == Token::Comma
        })
        .into_iter()
        .map(|(segment_start, segment_end)| parse_field_expr(tokens, segment_start, segment_end))
        .collect()
    } else {
        Vec::new()
    };
    Some((
        Expr::Update {
            base: Box::new(parse_expr(tokens, start + 1, with_idx.saturating_sub(1))),
            fields,
        },
        span_for_indices(tokens, start, end),
    ))
}

fn parse_atomic_base(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Option<(Spanned<Expr>, usize)> {
    if start > end {
        return None;
    }

    let token = &tokens[start].0;
    if let Some(literal) = token_as_literal(token) {
        return Some(((Expr::Literal(literal), tokens[start].1), start + 1));
    }

    match token {
        Token::Underscore => {
            return Some(((Expr::Ident("_".to_string()), tokens[start].1), start + 1));
        }
        Token::Id(name) if name == "config" => {
            let mut parts = Vec::new();
            let mut idx = start + 1;
            while idx <= end {
                let Some(part) = token_as_ident(&tokens[idx].0) else {
                    break;
                };
                parts.push((part, tokens[idx].1));
                idx += 1;
                if idx <= end && tokens[idx].0 == Token::Dot {
                    idx += 1;
                } else {
                    break;
                }
            }
            if !parts.is_empty() {
                return Some((
                    (
                        Expr::Config(parts),
                        span_for_indices(tokens, start, idx.saturating_sub(1)),
                    ),
                    idx,
                ));
            }
        }
        Token::TyVal(name) => {
            return Some(((Expr::TypeVar(name.clone()), tokens[start].1), start + 1));
        }
        Token::KwRef if start < end => {
            if let Some(name) = token_as_ident(&tokens[start + 1].0) {
                return Some((
                    (
                        Expr::Ref((name, tokens[start + 1].1)),
                        span_for_indices(tokens, start, start + 1),
                    ),
                    start + 2,
                ));
            }
        }
        Token::KwSizeof if start < end && tokens[start + 1].0 == Token::LeftBracket => {
            let close_idx = find_matching_round_bracket(tokens, start + 1)?;
            if close_idx <= end {
                return Some((
                    (
                        Expr::SizeOf(parse_type_expr(
                            tokens,
                            start + 2,
                            close_idx.saturating_sub(1),
                        )),
                        span_for_indices(tokens, start, close_idx),
                    ),
                    close_idx + 1,
                ));
            }
        }
        Token::KwConstraint if start < end && tokens[start + 1].0 == Token::LeftBracket => {
            let close_idx = find_matching_round_bracket(tokens, start + 1)?;
            if close_idx <= end {
                return Some((
                    (
                        Expr::Constraint(parse_type_expr(
                            tokens,
                            start + 2,
                            close_idx.saturating_sub(1),
                        )),
                        span_for_indices(tokens, start, close_idx),
                    ),
                    close_idx + 1,
                ));
            }
        }
        Token::KwAssert if start < end && tokens[start + 1].0 == Token::LeftBracket => {
            let close_idx = find_matching_round_bracket(tokens, start + 1)?;
            if close_idx <= end {
                let (arg_ranges, _) = if start + 2 < close_idx {
                    split_top_level_commas(tokens, start + 2, close_idx - 1)
                } else {
                    (Vec::new(), Vec::new())
                };
                let condition = arg_ranges
                    .first()
                    .map(|(arg_start, arg_end)| parse_expr(tokens, *arg_start, *arg_end))
                    .unwrap_or_else(|| error_expr(tokens, start + 2, close_idx.saturating_sub(1)));
                let message = arg_ranges
                    .get(1)
                    .map(|(arg_start, arg_end)| Box::new(parse_expr(tokens, *arg_start, *arg_end)));
                return Some((
                    (
                        Expr::Assert {
                            condition: Box::new(condition),
                            message,
                        },
                        span_for_indices(tokens, start, close_idx),
                    ),
                    close_idx + 1,
                ));
            }
        }
        Token::KwStruct => {
            let name_idx = if start < end && matches!(tokens[start + 1].0, Token::Id(_)) {
                Some(start + 1)
            } else {
                None
            };
            let brace_idx = name_idx.map_or(start + 1, |idx| idx + 1);
            if brace_idx <= end && tokens[brace_idx].0 == Token::LeftCurlyBracket {
                let close_idx = find_matching_bracket(tokens, brace_idx)?;
                if close_idx <= end {
                    let fields = if brace_idx + 1 < close_idx {
                        split_top_level_segments(tokens, brace_idx + 1, close_idx - 1, |token| {
                            *token == Token::Comma
                        })
                        .into_iter()
                        .map(|(segment_start, segment_end)| {
                            parse_field_expr(tokens, segment_start, segment_end)
                        })
                        .collect()
                    } else {
                        Vec::new()
                    };
                    return Some((
                        (
                            Expr::Struct {
                                name: name_idx.and_then(|idx| {
                                    token_as_ident(&tokens[idx].0).map(|name| (name, tokens[idx].1))
                                }),
                                fields,
                            },
                            span_for_indices(tokens, start, close_idx),
                        ),
                        close_idx + 1,
                    ));
                }
            }
        }
        Token::LeftCurlyBracket => {
            let close_idx = find_matching_bracket(tokens, start)?;
            if close_idx <= end {
                let expr = parse_update_expr(tokens, start, close_idx)
                    .unwrap_or_else(|| parse_block(tokens, start, close_idx));
                return Some((expr, close_idx + 1));
            }
        }
        Token::LeftBracket => {
            let close_idx = find_matching_round_bracket(tokens, start)?;
            if close_idx <= end {
                let expr = if start + 1 == close_idx {
                    (
                        Expr::Literal(Literal::Unit),
                        span_for_indices(tokens, start, close_idx),
                    )
                } else {
                    let segments =
                        split_top_level_segments(tokens, start + 1, close_idx - 1, |token| {
                            *token == Token::Comma
                        });
                    if segments.len() > 1 {
                        (
                            Expr::Tuple(
                                segments
                                    .into_iter()
                                    .map(|(segment_start, segment_end)| {
                                        parse_expr(tokens, segment_start, segment_end)
                                    })
                                    .collect(),
                            ),
                            span_for_indices(tokens, start, close_idx),
                        )
                    } else {
                        parse_expr(tokens, start + 1, close_idx - 1)
                    }
                };
                return Some((expr, close_idx + 1));
            }
        }
        Token::LeftSquareBracket => {
            let close_idx = find_matching_bracket(tokens, start)?;
            if close_idx <= end {
                let expr = if start + 1 == close_idx {
                    (
                        Expr::Vector(Vec::new()),
                        span_for_indices(tokens, start, close_idx),
                    )
                } else if let Some(with_idx) =
                    find_top_level_token(tokens, start + 1, close_idx - 1, |token| {
                        *token == Token::KwWith
                    })
                {
                    let span = span_for_indices(tokens, start, close_idx);
                    let updates = if with_idx < close_idx - 1 {
                        split_top_level_segments(tokens, with_idx + 1, close_idx - 1, |token| {
                            *token == Token::Comma
                        })
                        .into_iter()
                        .map(|(segment_start, segment_end)| {
                            parse_vector_update(tokens, segment_start, segment_end)
                        })
                        .collect()
                    } else {
                        Vec::new()
                    };
                    desugar_vector_update_expr(
                        parse_expr(tokens, start + 1, with_idx.saturating_sub(1)),
                        updates,
                        span,
                        tokens[start].1,
                        tokens[close_idx].1,
                    )
                } else {
                    let items =
                        split_top_level_segments(tokens, start + 1, close_idx - 1, |token| {
                            *token == Token::Comma
                        })
                        .into_iter()
                        .map(|(segment_start, segment_end)| {
                            parse_expr(tokens, segment_start, segment_end)
                        })
                        .collect();
                    (
                        Expr::Vector(items),
                        span_for_indices(tokens, start, close_idx),
                    )
                };
                return Some((expr, close_idx + 1));
            }
        }
        Token::LeftSquareBar => {
            let close_idx = find_matching_bracket(tokens, start)?;
            if close_idx <= end {
                let items = if start + 1 < close_idx {
                    split_top_level_segments(tokens, start + 1, close_idx - 1, |token| {
                        *token == Token::Comma
                    })
                    .into_iter()
                    .map(|(segment_start, segment_end)| {
                        parse_expr(tokens, segment_start, segment_end)
                    })
                    .collect()
                } else {
                    Vec::new()
                };
                return Some((
                    (
                        Expr::List(items),
                        span_for_indices(tokens, start, close_idx),
                    ),
                    close_idx + 1,
                ));
            }
        }
        _ => {}
    }

    if let Some(((name, name_span), next_idx)) = parse_name_with_optional_hash(tokens, start) {
        return Some(((Expr::Ident(name), name_span), next_idx));
    }

    None
}

fn modifier_call_receiver(expr: &Spanned<Expr>, via_arrow: bool) -> Option<Spanned<Expr>> {
    if via_arrow {
        match &expr.0 {
            Expr::Ident(name) => Some((Expr::Ref((name.clone(), expr.1)), expr.1)),
            _ => None,
        }
    } else {
        Some(expr.clone())
    }
}

fn unit_expr(span: Span) -> Spanned<Expr> {
    (Expr::Literal(Literal::Unit), span)
}

fn unit_pattern(span: Span) -> Spanned<Pattern> {
    (Pattern::Literal(Literal::Unit), span)
}

fn ident_call_expr(
    callee: Spanned<String>,
    args: Vec<Spanned<Expr>>,
    open_span: Span,
    close_span: Span,
    arg_separator_spans: Vec<Span>,
    expr_start: usize,
) -> Spanned<Expr> {
    (
        Expr::Call(Call {
            callee: Box::new((Expr::Ident(callee.0), callee.1)),
            args,
            open_span,
            close_span,
            arg_separator_spans,
        }),
        Span::new(expr_start, close_span.end),
    )
}

fn desugar_vector_update_expr(
    mut base: Spanned<Expr>,
    updates: Vec<Spanned<VectorUpdate>>,
    span: Span,
    open_span: Span,
    close_span: Span,
) -> Spanned<Expr> {
    for update in updates {
        let (callee_name, args) = match update.0 {
            VectorUpdate::Assignment { target, value } => {
                ("vector_update#".to_string(), vec![base, target, value])
            }
            VectorUpdate::RangeAssignment { start, end, value } => (
                "vector_update_subrange#".to_string(),
                vec![base, start, end, value],
            ),
            VectorUpdate::Shorthand(name) => {
                let ident = (Expr::Ident(name.0.clone()), name.1);
                (
                    "vector_update#".to_string(),
                    vec![base, ident.clone(), ident],
                )
            }
        };
        base = ident_call_expr(
            (callee_name, update.1),
            args,
            open_span,
            close_span,
            Vec::new(),
            span.start,
        );
    }

    base
}

fn modifier_call_expr(
    receiver: Spanned<Expr>,
    field: Spanned<String>,
    open_span: Span,
    close_span: Span,
    mut explicit_args: Vec<Spanned<Expr>>,
    arg_separator_spans: Vec<Span>,
) -> Spanned<Expr> {
    let receiver_start = receiver.1.start;
    let mut args = Vec::with_capacity(explicit_args.len() + 1);
    args.push(receiver);
    args.append(&mut explicit_args);
    (
        Expr::Call(Call {
            callee: Box::new((Expr::Ident(format!("_mod_{}", field.0)), field.1)),
            args,
            open_span,
            close_span,
            arg_separator_spans,
        }),
        Span::new(receiver_start, close_span.end),
    )
}

fn parse_prefixed_atomic_expr_with_end(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Option<(Spanned<Expr>, usize)> {
    let span = span_for_indices(tokens, start, end);
    let mut idx = start;
    let mut prefixes = Vec::new();

    while let Some((op, op_span, next_idx)) = prefix_op(tokens, idx, end) {
        prefixes.push((op, op_span));
        idx = next_idx;
    }

    let Some((mut expr, mut next_idx)) = parse_atomic_base(tokens, idx, end) else {
        return None;
    };

    while next_idx <= end {
        match tokens[next_idx].0 {
            Token::Unit => {
                // The lexer canonicalizes `()` into a single token, so `f()` reaches the
                // parser as `Id("f"), Unit` instead of a `(` ... `)` suffix pair.
                let unit_span = tokens[next_idx].1;
                let args = match expr.0 {
                    Expr::Ident(_) => vec![unit_expr(unit_span)],
                    _ => Vec::new(),
                };
                expr = (
                    Expr::Call(Call {
                        callee: Box::new(expr),
                        args,
                        open_span: unit_span,
                        close_span: unit_span,
                        arg_separator_spans: Vec::new(),
                    }),
                    span_for_indices(tokens, idx, next_idx),
                );
                next_idx += 1;
            }
            Token::LeftBracket => {
                let Some(close_idx) = find_matching_round_bracket(tokens, next_idx) else {
                    return None;
                };
                if close_idx > end {
                    return None;
                }
                let (arg_ranges, separators) = if next_idx + 1 < close_idx {
                    split_top_level_commas(tokens, next_idx + 1, close_idx - 1)
                } else {
                    (Vec::new(), Vec::new())
                };
                let args = arg_ranges
                    .into_iter()
                    .map(|(arg_start, arg_end)| parse_expr(tokens, arg_start, arg_end))
                    .collect();
                let combined_span = span_for_indices(tokens, idx, close_idx);
                expr = (
                    Expr::Call(Call {
                        callee: Box::new(expr),
                        args,
                        open_span: tokens[next_idx].1,
                        close_span: tokens[close_idx].1,
                        arg_separator_spans: separators,
                    }),
                    combined_span,
                );
                next_idx = close_idx + 1;
            }
            Token::Dot | Token::RightArrow => {
                let via_arrow = tokens[next_idx].0 == Token::RightArrow;
                if next_idx + 1 > end {
                    return None;
                }
                let Some(field_name) = token_as_ident(&tokens[next_idx + 1].0) else {
                    return None;
                };
                let field_span = tokens[next_idx + 1].1;
                let field = (field_name, field_span);
                if next_idx + 2 <= end {
                    match tokens[next_idx + 2].0 {
                        Token::Unit => {
                            if let Some(receiver) = modifier_call_receiver(&expr, via_arrow) {
                                let unit_span = tokens[next_idx + 2].1;
                                expr = modifier_call_expr(
                                    receiver,
                                    field,
                                    unit_span,
                                    unit_span,
                                    Vec::new(),
                                    Vec::new(),
                                );
                                next_idx += 3;
                                continue;
                            }
                        }
                        Token::LeftBracket => {
                            let Some(close_idx) = find_matching_round_bracket(tokens, next_idx + 2)
                            else {
                                return None;
                            };
                            if close_idx > end {
                                return None;
                            }
                            if let Some(receiver) = modifier_call_receiver(&expr, via_arrow) {
                                let (arg_ranges, separators) = if next_idx + 3 < close_idx {
                                    split_top_level_commas(tokens, next_idx + 3, close_idx - 1)
                                } else {
                                    (Vec::new(), Vec::new())
                                };
                                let args = arg_ranges
                                    .into_iter()
                                    .map(|(arg_start, arg_end)| {
                                        parse_expr(tokens, arg_start, arg_end)
                                    })
                                    .collect();
                                expr = modifier_call_expr(
                                    receiver,
                                    field,
                                    tokens[next_idx + 2].1,
                                    tokens[close_idx].1,
                                    args,
                                    separators,
                                );
                                next_idx = close_idx + 1;
                                continue;
                            }
                        }
                        _ => {}
                    }
                }
                expr = (
                    Expr::Field {
                        expr: Box::new(expr),
                        field,
                        via_arrow,
                    },
                    span_for_indices(tokens, idx, next_idx + 1),
                );
                next_idx += 2;
            }
            Token::LeftSquareBracket => {
                let Some(close_idx) = find_matching_bracket(tokens, next_idx) else {
                    return None;
                };
                if close_idx > end || close_idx <= next_idx {
                    return None;
                }
                let inner_start = next_idx + 1;
                let inner_end = close_idx - 1;
                let expr_start = expr.1.start;
                let next_expr = if let Some(comma_idx) =
                    find_top_level_token(tokens, inner_start, inner_end, |token| {
                        *token == Token::Comma
                    }) {
                    ident_call_expr(
                        ("slice".to_string(), tokens[next_idx].1),
                        vec![
                            expr,
                            parse_expr(tokens, inner_start, comma_idx.saturating_sub(1)),
                            parse_expr(tokens, comma_idx + 1, inner_end),
                        ],
                        tokens[next_idx].1,
                        tokens[close_idx].1,
                        vec![tokens[comma_idx].1],
                        expr_start,
                    )
                } else if let Some(dd_idx) =
                    find_top_level_double_dot(tokens, inner_start, inner_end)
                {
                    ident_call_expr(
                        ("vector_subrange#".to_string(), tokens[next_idx].1),
                        vec![
                            expr,
                            parse_expr(tokens, inner_start, dd_idx.saturating_sub(1)),
                            parse_expr(tokens, dd_idx + 2, inner_end),
                        ],
                        tokens[next_idx].1,
                        tokens[close_idx].1,
                        Vec::new(),
                        expr_start,
                    )
                } else {
                    ident_call_expr(
                        ("vector_access#".to_string(), tokens[next_idx].1),
                        vec![expr, parse_expr(tokens, inner_start, inner_end)],
                        tokens[next_idx].1,
                        tokens[close_idx].1,
                        Vec::new(),
                        expr_start,
                    )
                };
                expr = next_expr;
                next_idx = close_idx + 1;
            }
            Token::Colon => {
                if next_idx == end {
                    return None;
                }
                expr = (
                    Expr::Cast {
                        expr: Box::new(expr),
                        ty: parse_type_expr(tokens, next_idx + 1, end),
                    },
                    span,
                );
                next_idx = end + 1;
            }
            _ => break,
        }
    }

    for (op, op_span) in prefixes.into_iter().rev() {
        expr = (
            Expr::Prefix {
                op: (op, op_span),
                expr: Box::new(expr),
            },
            span,
        );
    }

    Some((expr, next_idx))
}

fn parse_prefixed_atomic_expr(tokens: &[(Token, Span)], start: usize, end: usize) -> Spanned<Expr> {
    match parse_prefixed_atomic_expr_with_end(tokens, start, end) {
        Some((expr, next_idx)) if next_idx > end => expr,
        _ => error_expr(tokens, start, end),
    }
}

fn parse_foreach_header(
    tokens: &[(Token, Span)],
    open_idx: usize,
    close_idx: usize,
) -> Option<ForeachExpr> {
    if open_idx + 1 >= close_idx {
        return None;
    }

    let mut cursor = open_idx + 1;
    let iterator = token_as_ident(&tokens.get(cursor)?.0)?;
    let iterator_span = tokens[cursor].1;
    cursor += 1;

    let start_keyword = token_as_ident(&tokens.get(cursor)?.0)?;
    let start_keyword_span = tokens[cursor].1;
    cursor += 1;

    let (start_expr, next_cursor) =
        parse_prefixed_atomic_expr_with_end(tokens, cursor, close_idx.saturating_sub(1))?;
    cursor = next_cursor;
    if cursor >= close_idx {
        return None;
    }

    let end_keyword = token_as_ident(&tokens.get(cursor)?.0)?;
    let end_keyword_span = tokens[cursor].1;
    cursor += 1;
    if cursor >= close_idx {
        return None;
    }

    let by_idx = find_top_level_token(tokens, cursor, close_idx - 1, |token| *token == Token::KwBy);
    let in_idx = find_top_level_token(tokens, cursor, close_idx - 1, |token| *token == Token::KwIn);
    let end_expr_end = by_idx
        .or(in_idx)
        .map_or(close_idx - 1, |idx| idx.saturating_sub(1));
    let end_expr = parse_expr(tokens, cursor, end_expr_end);

    let step = by_idx.and_then(|by_idx| {
        let step_end = in_idx
            .filter(|in_idx| *in_idx > by_idx)
            .map_or(close_idx - 1, |in_idx| in_idx.saturating_sub(1));
        (by_idx + 1 <= step_end).then(|| Box::new(parse_expr(tokens, by_idx + 1, step_end)))
    });
    let ty = in_idx.and_then(|in_idx| {
        (in_idx + 1 < close_idx).then(|| parse_type_expr(tokens, in_idx + 1, close_idx - 1))
    });

    Some(ForeachExpr {
        iterator: (iterator, iterator_span),
        start_keyword: (start_keyword, start_keyword_span),
        start: Box::new(start_expr),
        end_keyword: (end_keyword, end_keyword_span),
        end: Box::new(end_expr),
        step,
        ty,
        header_span: span_for_indices(tokens, open_idx.saturating_sub(1), close_idx),
        body: Box::new(error_expr(tokens, close_idx, close_idx)),
    })
}

fn parse_exp0(tokens: &[(Token, Span)], start: usize, end: usize) -> Spanned<Expr> {
    let span = span_for_indices(tokens, start, end);
    let mut segments = Vec::new();
    let mut ops = Vec::new();
    let mut depth = 0_i32;
    let mut segment_start = start;
    let mut idx = start;
    while idx <= end {
        let token = &tokens[idx].0;
        if token_is_open_bracket(token) {
            depth += 1;
        } else if token_is_close_bracket(token) {
            depth -= 1;
        }

        if depth == 0 {
            if let Some((op, op_span, next_idx)) = match_composite_infix_op(tokens, idx, end) {
                if idx > segment_start {
                    segments.push((segment_start, idx - 1));
                    ops.push((op, op_span));
                    segment_start = next_idx;
                    idx = next_idx;
                    continue;
                }
            }
        }

        let shift_op = depth == 0
            && idx < end
            && matches!(
                (&tokens[idx].0, &tokens[idx + 1].0),
                (Token::LessThan, Token::LessThan) | (Token::GreaterThan, Token::GreaterThan)
            );

        if shift_op {
            if idx > segment_start {
                segments.push((segment_start, idx - 1));
                ops.push((
                    format!("{}{}", tokens[idx].0, tokens[idx + 1].0),
                    span_for_indices(tokens, idx, idx + 1),
                ));
                segment_start = idx + 2;
            }
            idx += 2;
            continue;
        }

        if depth == 0 && idx > segment_start && is_exp_op(token) {
            segments.push((segment_start, idx - 1));
            ops.push((tokens[idx].0.to_string(), tokens[idx].1));
            segment_start = idx + 1;
        }
        idx += 1;
    }
    segments.push((segment_start, end));

    if ops.is_empty() {
        return parse_prefixed_atomic_expr(tokens, start, end);
    }

    let mut expr = parse_prefixed_atomic_expr(tokens, segments[0].0, segments[0].1);
    for ((op, op_span), (segment_start, segment_end)) in
        ops.into_iter().zip(segments.into_iter().skip(1))
    {
        let rhs = parse_prefixed_atomic_expr(tokens, segment_start, segment_end);
        expr = (
            Expr::Infix {
                lhs: Box::new(expr),
                op: (op, op_span),
                rhs: Box::new(rhs),
            },
            span,
        );
    }

    expr
}

fn parse_measure_expr(
    tokens: &[(Token, Span)],
    cursor: &mut usize,
    end: usize,
) -> Option<Box<Spanned<Expr>>> {
    if *cursor >= end || tokens[*cursor].0 != Token::KwTerminationMeasure {
        return None;
    }
    if *cursor + 1 > end || tokens[*cursor + 1].0 != Token::LeftCurlyBracket {
        return None;
    }
    let brace_idx = *cursor + 1;
    let close_idx = find_matching_bracket(tokens, brace_idx)?;
    let parsed = Box::new(parse_expr(
        tokens,
        brace_idx + 1,
        close_idx.saturating_sub(1),
    ));
    *cursor = close_idx + 1;
    Some(parsed)
}

fn parse_expr(tokens: &[(Token, Span)], start: usize, end: usize) -> Spanned<Expr> {
    if start > end {
        return (Expr::Error(String::new()), fallback_span(tokens, start));
    }
    let span = span_for_indices(tokens, start, end);

    if let Some((attr, next_idx)) = parse_attribute_prefix(tokens, start) {
        if next_idx <= end {
            return (
                Expr::Attribute {
                    attr,
                    expr: Box::new(parse_expr(tokens, next_idx, end)),
                },
                span,
            );
        }
    }

    if tokens[start].0 == Token::LeftCurlyBracket
        && find_matching_bracket(tokens, start) == Some(end)
    {
        if let Some(update) = parse_update_expr(tokens, start, end) {
            return update;
        }
        return parse_block(tokens, start, end);
    }

    match tokens[start].0 {
        Token::KwLet => {
            if let Some(in_idx) =
                find_top_level_token(tokens, start + 1, end, |token| *token == Token::KwIn)
            {
                if start + 1 < in_idx {
                    return (
                        Expr::Let {
                            binding: parse_let_binding(tokens, start + 1, in_idx.saturating_sub(1)),
                            body: Box::new(parse_expr(tokens, in_idx + 1, end)),
                        },
                        span,
                    );
                }
            }
        }
        Token::KwVar => {
            if let Some(eq_idx) = find_top_level_assignment_token(tokens, start + 1, end) {
                if let Some(in_idx) =
                    find_top_level_token(tokens, eq_idx + 1, end, |token| *token == Token::KwIn)
                {
                    return (
                        Expr::Var {
                            target: Box::new(parse_prefixed_atomic_expr(
                                tokens,
                                start + 1,
                                eq_idx.saturating_sub(1),
                            )),
                            value: Box::new(parse_expr(
                                tokens,
                                eq_idx + 1,
                                in_idx.saturating_sub(1),
                            )),
                            body: Box::new(parse_expr(tokens, in_idx + 1, end)),
                        },
                        span,
                    );
                }
            }
        }
        Token::KwSwitch => {
            if let Some(brace_idx) =
                find_top_level_token_including_opens(tokens, start + 1, end, |token| {
                    *token == Token::LeftCurlyBracket
                })
            {
                if find_matching_bracket(tokens, brace_idx) == Some(end) {
                    let cases = if brace_idx + 1 < end {
                        split_switch_cases(tokens, brace_idx + 1, end - 1)
                            .into_iter()
                            .map(|(segment_start, segment_end)| {
                                parse_case(tokens, segment_start, segment_end)
                            })
                            .collect()
                    } else {
                        Vec::new()
                    };
                    return (
                        Expr::Match {
                            scrutinee: Box::new(parse_expr(
                                tokens,
                                start + 1,
                                brace_idx.saturating_sub(1),
                            )),
                            cases,
                        },
                        span,
                    );
                }
            }
        }
        Token::KwReturn if start < end => {
            return (
                Expr::Return(Box::new(parse_expr(tokens, start + 1, end))),
                span,
            );
        }
        Token::KwThrow if start < end => {
            return (
                Expr::Throw(Box::new(parse_expr(tokens, start + 1, end))),
                span,
            );
        }
        Token::KwExit => {
            return (
                Expr::Exit((start < end).then(|| Box::new(parse_expr(tokens, start + 1, end)))),
                span,
            );
        }
        Token::KwIf => {
            if let Some(then_idx) =
                find_top_level_token(tokens, start + 1, end, |token| *token == Token::KwThen)
            {
                let else_idx = find_matching_else_token(tokens, then_idx + 1, end);
                let then_end = else_idx.map_or(end, |idx| idx.saturating_sub(1));
                return (
                    Expr::If {
                        cond: Box::new(parse_expr(tokens, start + 1, then_idx.saturating_sub(1))),
                        then_branch: Box::new(parse_expr(tokens, then_idx + 1, then_end)),
                        else_branch: else_idx.map(|idx| Box::new(parse_expr(tokens, idx + 1, end))),
                    },
                    span,
                );
            }
        }
        Token::KwMatch => {
            if let Some(brace_idx) =
                find_top_level_token_including_opens(tokens, start + 1, end, |token| {
                    *token == Token::LeftCurlyBracket
                })
            {
                if find_matching_bracket(tokens, brace_idx) == Some(end) {
                    let cases = if brace_idx + 1 < end {
                        split_top_level_segments(tokens, brace_idx + 1, end - 1, |token| {
                            *token == Token::Comma
                        })
                        .into_iter()
                        .map(|(segment_start, segment_end)| {
                            parse_case(tokens, segment_start, segment_end)
                        })
                        .collect()
                    } else {
                        Vec::new()
                    };
                    return (
                        Expr::Match {
                            scrutinee: Box::new(parse_expr(
                                tokens,
                                start + 1,
                                brace_idx.saturating_sub(1),
                            )),
                            cases,
                        },
                        span,
                    );
                }
            }
        }
        Token::KwTry => {
            if let Some(catch_idx) =
                find_top_level_token(tokens, start + 1, end, |token| *token == Token::KwCatch)
            {
                if let Some(brace_idx) =
                    find_top_level_token_including_opens(tokens, catch_idx + 1, end, |token| {
                        *token == Token::LeftCurlyBracket
                    })
                {
                    if find_matching_bracket(tokens, brace_idx) == Some(end) {
                        let cases = if brace_idx + 1 < end {
                            split_top_level_segments(tokens, brace_idx + 1, end - 1, |token| {
                                *token == Token::Comma
                            })
                            .into_iter()
                            .map(|(segment_start, segment_end)| {
                                parse_case(tokens, segment_start, segment_end)
                            })
                            .collect()
                        } else {
                            Vec::new()
                        };
                        return (
                            Expr::Try {
                                scrutinee: Box::new(parse_expr(
                                    tokens,
                                    start + 1,
                                    catch_idx.saturating_sub(1),
                                )),
                                cases,
                            },
                            span,
                        );
                    }
                }
            }
        }
        Token::KwForeach => {
            if start < end && tokens[start + 1].0 == Token::LeftBracket {
                if let Some(close_idx) = find_matching_round_bracket(tokens, start + 1) {
                    if close_idx < end {
                        if let Some(mut foreach) =
                            parse_foreach_header(tokens, start + 1, close_idx)
                        {
                            foreach.header_span = span_for_indices(tokens, start, close_idx);
                            foreach.body = Box::new(parse_expr(tokens, close_idx + 1, end));
                            return (Expr::Foreach(foreach), span);
                        }
                    }
                }
            }
        }
        Token::KwRepeat => {
            let mut cursor = start + 1;
            let measure = parse_measure_expr(tokens, &mut cursor, end);
            if let Some(until_idx) =
                find_top_level_token(tokens, cursor, end, |token| *token == Token::KwUntil)
            {
                if cursor < until_idx && until_idx < end {
                    return (
                        Expr::Repeat {
                            measure,
                            body: Box::new(parse_expr(tokens, cursor, until_idx.saturating_sub(1))),
                            until: Box::new(parse_expr(tokens, until_idx + 1, end)),
                        },
                        span,
                    );
                }
            }
        }
        Token::KwWhile => {
            let mut cursor = start + 1;
            let measure = parse_measure_expr(tokens, &mut cursor, end);
            if let Some(do_idx) =
                find_top_level_token(tokens, cursor, end, |token| *token == Token::KwDo)
            {
                if cursor < do_idx && do_idx < end {
                    return (
                        Expr::While {
                            measure,
                            cond: Box::new(parse_expr(tokens, cursor, do_idx.saturating_sub(1))),
                            body: Box::new(parse_expr(tokens, do_idx + 1, end)),
                        },
                        span,
                    );
                }
            }
        }
        _ => {}
    }

    if let Some(eq_idx) = find_top_level_assignment_token(tokens, start, end) {
        if eq_idx < end {
            return (
                Expr::Assign {
                    lhs: Box::new(parse_exp0(tokens, start, eq_idx.saturating_sub(1))),
                    rhs: Box::new(parse_expr(tokens, eq_idx + 1, end)),
                },
                span,
            );
        }
    }

    parse_exp0(tokens, start, end)
}

fn parse_mapping_arm(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Option<Spanned<MappingArm>> {
    if start > end {
        return None;
    }

    let span = span_for_indices(tokens, start, end);
    let (direction, cursor, arrow_token) = match tokens[start].0 {
        Token::KwForwards => (
            MappingArmDirection::Forwards,
            start + 1,
            Token::FatRightArrow,
        ),
        Token::KwBackwards => (
            MappingArmDirection::Backwards,
            start + 1,
            Token::FatRightArrow,
        ),
        _ => (
            MappingArmDirection::Bidirectional,
            start,
            Token::DoubleArrow,
        ),
    };

    if cursor > end {
        return None;
    }

    let arrow_idx = find_top_level_token(tokens, cursor, end, |token| *token == arrow_token)?;
    if arrow_idx == cursor || arrow_idx >= end {
        return None;
    }

    let mut guard = None;
    let lhs_end = if let Some(if_idx) =
        find_top_level_token(tokens, cursor, arrow_idx.saturating_sub(1), |token| {
            *token == Token::KwIf
        }) {
        if if_idx >= arrow_idx {
            return None;
        }
        guard = Some(parse_expr(tokens, if_idx + 1, arrow_idx.saturating_sub(1)));
        if_idx.saturating_sub(1)
    } else {
        arrow_idx.saturating_sub(1)
    };

    if cursor > lhs_end {
        return None;
    }

    let mut rhs_end = end;
    if let Some(when_idx) = find_top_level_token(tokens, arrow_idx + 1, end, |token| {
        *token == Token::KwWhen || token_is_ident_name(token, "when")
    }) {
        if when_idx < end && guard.is_none() {
            guard = Some(parse_expr(tokens, when_idx + 1, end));
        }
        rhs_end = when_idx.saturating_sub(1);
    }

    if arrow_idx + 1 > rhs_end {
        return None;
    }

    Some((
        MappingArm {
            direction,
            lhs: Box::new(parse_expr(tokens, cursor, lhs_end)),
            rhs: Box::new(parse_expr(tokens, arrow_idx + 1, rhs_end)),
            lhs_pattern: pattern_if_structured(tokens, cursor, lhs_end),
            rhs_pattern: pattern_if_structured(tokens, arrow_idx + 1, rhs_end),
            guard,
            arrow_span: tokens[arrow_idx].1,
        },
        span,
    ))
}

fn parse_mapping_body(tokens: &[(Token, Span)], start: usize, end: usize) -> Option<MappingBody> {
    let arms = if tokens[start].0 == Token::LeftCurlyBracket
        && find_matching_bracket(tokens, start) == Some(end)
    {
        if start + 1 < end {
            split_top_level_segments(tokens, start + 1, end - 1, |token| *token == Token::Comma)
                .into_iter()
                .filter_map(|(segment_start, segment_end)| {
                    parse_mapping_arm(tokens, segment_start, segment_end)
                })
                .collect()
        } else {
            Vec::new()
        }
    } else {
        vec![parse_mapping_arm(tokens, start, end)?]
    };

    Some(MappingBody { arms })
}

fn parse_mapping_def(
    tokens: &[(Token, Span)],
    start_idx: usize,
) -> Option<(Spanned<TopLevelDef>, usize)> {
    let name_idx = start_idx + 1;
    let name = token_as_ident(&tokens.get(name_idx)?.0)?;
    let item_end = find_declaration_end(tokens, start_idx);
    let eq_idx = find_top_level_token(tokens, name_idx + 1, item_end, |token| {
        *token == Token::Equal
    })?;
    let signature = find_top_level_token(tokens, name_idx + 1, eq_idx.saturating_sub(1), |token| {
        *token == Token::Colon
    })
    .and_then(|colon_idx| {
        (colon_idx < eq_idx.saturating_sub(1))
            .then(|| parse_type_expr(tokens, colon_idx + 1, eq_idx - 1))
    });
    let mapping_body = (eq_idx < item_end)
        .then(|| parse_mapping_body(tokens, eq_idx + 1, item_end))
        .flatten();
    let body_span = (eq_idx < item_end).then(|| span_for_indices(tokens, eq_idx + 1, item_end));

    Some((
        (
            TopLevelDef::CallableDef(CallableDef {
                modifiers: DefModifiers::default(),
                kind: CallableDefKind::Mapping,
                name: (name, tokens[name_idx].1),
                signature,
                rec_measure: None,
                clauses: Vec::new(),
                params: Vec::new(),
                return_ty: None,
                body: None,
                mapping_body,
                body_span,
            }),
            span_for_indices(tokens, start_idx, item_end),
        ),
        item_end + 1,
    ))
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

        if depth == 0 && (*token == Token::Semicolon || declaration_starts_at(tokens, idx)) {
            break;
        }
        end_idx = idx;
    }
    end_idx
}

fn skip_outcome_item(tokens: &[(Token, Span)], start_idx: usize) -> usize {
    let header_end = find_top_level_token(
        tokens,
        start_idx + 1,
        tokens.len().saturating_sub(1),
        |token| *token == Token::Equal,
    );
    if let Some(eq_idx) = header_end {
        if matches!(tokens.get(eq_idx + 1), Some((Token::LeftCurlyBracket, _))) {
            if let Some(close_idx) = find_matching_bracket(tokens, eq_idx + 1) {
                return close_idx + 1;
            }
        }
    }
    find_declaration_end(tokens, start_idx) + 1
}

fn parse_param_list(
    tokens: &[(Token, Span)],
    open_idx: usize,
    close_idx: usize,
) -> Vec<Spanned<Pattern>> {
    if open_idx + 1 >= close_idx {
        return Vec::new();
    }

    split_top_level_segments(tokens, open_idx + 1, close_idx - 1, |token| {
        *token == Token::Comma
    })
    .into_iter()
    .map(|(segment_start, segment_end)| parse_pattern(tokens, segment_start, segment_end))
    .collect()
}

fn parse_spaced_params(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Vec<Spanned<Pattern>> {
    if start > end {
        return Vec::new();
    }

    (start..=end)
        .filter_map(|idx| match &tokens[idx].0 {
            Token::Id(name) => Some((Pattern::Ident(name.clone()), tokens[idx].1)),
            Token::Underscore => Some((Pattern::Wild, tokens[idx].1)),
            _ => None,
        })
        .collect()
}

fn parse_clause_patterns(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Vec<Spanned<Pattern>> {
    if start > end {
        return Vec::new();
    }

    if tokens[start].0 == Token::LeftBracket
        && find_matching_round_bracket(tokens, start) == Some(end)
    {
        return parse_param_list(tokens, start, end);
    }

    let spaced = parse_spaced_params(tokens, start, end);
    if !spaced.is_empty()
        && spaced.len() == end - start + 1
        && (start..=end).all(|idx| {
            matches!(
                tokens[idx].0,
                Token::Id(_) | Token::Underscore | Token::TyVal(_)
            )
        })
    {
        return spaced;
    }

    vec![parse_pattern(tokens, start, end)]
}

fn parse_quantifier_var(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Option<QuantifierVar> {
    if start > end {
        return None;
    }

    if start == end {
        if let Token::TyVal(name) = &tokens[start].0 {
            return Some(QuantifierVar {
                name: (name.clone(), tokens[start].1),
                kind: None,
                is_constant: false,
            });
        }
        return None;
    }

    if tokens[start].0 != Token::LeftBracket
        || find_matching_round_bracket(tokens, start) != Some(end)
    {
        return None;
    }

    let mut cursor = start + 1;
    let mut is_constant = false;
    if matches!(tokens.get(cursor), Some((Token::KwConstant, _))) {
        is_constant = true;
        cursor += 1;
    }
    let Token::TyVal(name) = &tokens.get(cursor)?.0 else {
        return None;
    };
    let name_span = tokens[cursor].1;
    cursor += 1;

    let kind = if cursor < end && tokens[cursor].0 == Token::Colon && cursor < end {
        (cursor < end).then(|| {
            (
                tokens_text(tokens, cursor + 1, end - 1),
                span_for_indices(tokens, cursor + 1, end - 1),
            )
        })
    } else {
        None
    };

    Some(QuantifierVar {
        name: (name.clone(), name_span),
        kind,
        is_constant,
    })
}

fn parse_callable_quantifier(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Option<CallableQuantifier> {
    if start > end {
        return None;
    }

    let mut vars = Vec::new();
    let mut cursor = start;
    let mut constraint_start = None;

    while cursor <= end {
        if tokens[cursor].0 == Token::Comma {
            cursor += 1;
            continue;
        }

        let group_end = if matches!(tokens[cursor].0, Token::TyVal(_)) {
            cursor
        } else if tokens[cursor].0 == Token::LeftBracket {
            let close_idx = find_matching_round_bracket(tokens, cursor)?;
            if close_idx > end {
                break;
            }
            close_idx
        } else {
            constraint_start = Some(cursor);
            break;
        };

        let Some(var) = parse_quantifier_var(tokens, cursor, group_end) else {
            constraint_start = Some(cursor);
            break;
        };
        let after_group = group_end + 1;
        if after_group <= end
            && tokens[after_group].0 != Token::Comma
            && !matches!(tokens[after_group].0, Token::TyVal(_) | Token::LeftBracket)
        {
            constraint_start = Some(cursor);
            break;
        }

        vars.push(var);
        cursor = after_group;
    }

    if vars.is_empty() {
        return None;
    }

    Some(CallableQuantifier {
        vars,
        constraint: constraint_start
            .map(|constraint_start| parse_type_expr(tokens, constraint_start, end)),
    })
}

fn parse_rec_measure(
    tokens: &[(Token, Span)],
    start_idx: usize,
) -> Option<(Spanned<RecMeasure>, usize)> {
    if tokens.get(start_idx)?.0 != Token::LeftCurlyBracket {
        return None;
    }
    let close_idx = find_matching_bracket(tokens, start_idx)?;
    let arrow_idx = find_top_level_token(
        tokens,
        start_idx + 1,
        close_idx.saturating_sub(1),
        |token| *token == Token::FatRightArrow,
    )?;
    if arrow_idx <= start_idx + 1 || arrow_idx >= close_idx.saturating_sub(1) {
        return None;
    }

    Some((
        (
            RecMeasure {
                pattern: parse_pattern(tokens, start_idx + 1, arrow_idx - 1),
                body: parse_expr(tokens, arrow_idx + 1, close_idx - 1),
            },
            span_for_indices(tokens, start_idx, close_idx),
        ),
        close_idx + 1,
    ))
}

fn looks_like_callable_clause_start(tokens: &[(Token, Span)], start: usize, end: usize) -> bool {
    if start > end {
        return false;
    }

    let (_, cursor) = parse_def_modifiers(tokens, start);
    let Some((_, next_cursor)) = parse_name_with_optional_hash(tokens, cursor) else {
        return false;
    };
    if next_cursor > end + 1 {
        return false;
    }

    find_top_level_token(tokens, next_cursor, end, |token| *token == Token::Equal).is_some()
}

fn find_callable_clause_end(tokens: &[(Token, Span)], eq_idx: usize) -> (usize, Option<usize>) {
    let mut depth = 0_i32;
    let mut end_idx = eq_idx;

    for idx in (eq_idx + 1)..tokens.len() {
        let token = &tokens[idx].0;
        if token_is_open_bracket(token) {
            depth += 1;
        } else if token_is_close_bracket(token) {
            depth = (depth - 1).max(0);
        }

        if depth == 0
            && *token == Token::KwAnd
            && looks_like_callable_clause_start(tokens, idx + 1, tokens.len().saturating_sub(1))
        {
            return (idx.saturating_sub(1), Some(idx + 1));
        }

        let body_intro_let = idx == eq_idx + 1 && matches!(token, Token::KwLet | Token::KwVar);
        if depth == 0
            && !body_intro_let
            && (*token == Token::Semicolon
                || token_starts_body_terminator(token)
                || declaration_starts_at(tokens, idx))
        {
            return (idx.saturating_sub(1), None);
        }

        end_idx = idx;
    }

    (end_idx, None)
}

fn parse_callable_clause(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
    kind: CallableDefKind,
) -> Option<Spanned<CallableClause>> {
    let span = span_for_indices(tokens, start, end);
    let (modifiers, mut cursor) = parse_def_modifiers(tokens, start);
    let ((name, name_span), next_cursor) = parse_name_with_optional_hash(tokens, cursor)?;
    cursor = next_cursor;

    let eq_idx = find_top_level_token(tokens, cursor, end, |token| *token == Token::Equal)?;
    if eq_idx >= end {
        return None;
    }
    let header_end = eq_idx.saturating_sub(1);

    let quantifier = if cursor <= header_end && tokens[cursor].0 == Token::KwForall {
        let dot_idx =
            find_top_level_token(tokens, cursor + 1, header_end, |token| *token == Token::Dot)?;
        let quantifier = parse_callable_quantifier(tokens, cursor + 1, dot_idx.saturating_sub(1));
        cursor = dot_idx + 1;
        quantifier
    } else {
        None
    };

    let arrow_idx = if cursor <= header_end {
        find_top_level_token(tokens, cursor, header_end, |token| {
            *token == Token::RightArrow
        })
    } else {
        None
    };
    let pattern_end = arrow_idx.map_or(header_end, |idx| idx.saturating_sub(1));

    let (patterns, guard) = if cursor <= pattern_end
        && tokens[cursor].0 == Token::LeftBracket
        && find_matching_round_bracket(tokens, cursor) == Some(pattern_end)
    {
        if let Some(if_idx) =
            find_top_level_token(tokens, cursor + 1, pattern_end.saturating_sub(1), |token| {
                *token == Token::KwIf
            })
        {
            (
                vec![parse_pattern(tokens, cursor + 1, if_idx.saturating_sub(1))],
                Some(parse_expr(tokens, if_idx + 1, pattern_end - 1)),
            )
        } else {
            (parse_clause_patterns(tokens, cursor, pattern_end), None)
        }
    } else {
        (parse_clause_patterns(tokens, cursor, pattern_end), None)
    };

    let return_ty = arrow_idx.and_then(|arrow_idx| {
        (arrow_idx < header_end).then(|| parse_type_expr(tokens, arrow_idx + 1, header_end))
    });
    let body_span = Some(span_for_indices(tokens, eq_idx + 1, end));
    let body = if matches!(
        kind,
        CallableDefKind::Function | CallableDefKind::FunctionClause
    ) {
        Some(parse_expr(tokens, eq_idx + 1, end))
    } else {
        None
    };
    let mapping_body = if matches!(
        kind,
        CallableDefKind::Mapping | CallableDefKind::MappingClause
    ) {
        parse_mapping_body(tokens, eq_idx + 1, end)
    } else {
        None
    };

    Some((
        CallableClause {
            modifiers,
            name: (name, name_span),
            patterns,
            guard,
            quantifier,
            return_ty,
            body,
            mapping_body,
            body_span,
        },
        span,
    ))
}

fn parse_callable_def(
    tokens: &[(Token, Span)],
    start_idx: usize,
    kind: CallableDefKind,
) -> Option<(Spanned<TopLevelDef>, usize)> {
    let mut cursor = start_idx + 1;
    let kind = match kind {
        CallableDefKind::Function | CallableDefKind::FunctionClause => {
            if tokens.get(cursor).map(|(token, _)| token) == Some(&Token::KwClause) {
                cursor += 1;
                CallableDefKind::FunctionClause
            } else {
                CallableDefKind::Function
            }
        }
        CallableDefKind::Mapping | CallableDefKind::MappingClause => {
            if tokens.get(cursor).map(|(token, _)| token) == Some(&Token::KwClause) {
                cursor += 1;
                CallableDefKind::MappingClause
            } else {
                CallableDefKind::Mapping
            }
        }
    };

    let rec_measure = if matches!(kind, CallableDefKind::Function)
        && matches!(tokens.get(cursor), Some((Token::LeftCurlyBracket, _)))
    {
        let (measure, next_cursor) = parse_rec_measure(tokens, cursor)?;
        cursor = next_cursor;
        Some(measure)
    } else {
        None
    };

    let mut clauses = Vec::new();
    let mut item_end = cursor;
    let mut clause_start = cursor;

    loop {
        let Some(eq_idx) = find_top_level_token(
            tokens,
            clause_start + 1,
            tokens.len().saturating_sub(1),
            |token| *token == Token::Equal,
        ) else {
            break;
        };
        let (clause_end, next_clause_start) = find_callable_clause_end(tokens, eq_idx);
        item_end = clause_end;
        let Some(clause) = parse_callable_clause(tokens, clause_start, clause_end, kind) else {
            break;
        };
        clauses.push(clause);
        let Some(next_clause_start) = next_clause_start else {
            break;
        };
        clause_start = next_clause_start;
    }

    let first_clause = clauses.first()?;
    let params = first_clause.0.patterns.clone();
    let return_ty = first_clause.0.return_ty.clone();
    let body = first_clause.0.body.clone();
    let mapping_body = first_clause.0.mapping_body.clone();
    let body_span = first_clause.0.body_span;
    let name = first_clause.0.name.clone();

    Some((
        (
            TopLevelDef::CallableDef(CallableDef {
                modifiers: DefModifiers::default(),
                kind,
                name,
                signature: None,
                rec_measure,
                clauses,
                params,
                return_ty,
                body,
                mapping_body,
                body_span,
            }),
            span_for_indices(tokens, start_idx, item_end),
        ),
        item_end + 1,
    ))
}

fn parse_callable_spec(
    tokens: &[(Token, Span)],
    start_idx: usize,
    kind: CallableSpecKind,
) -> Option<(Spanned<TopLevelDef>, usize)> {
    let name_idx = start_idx + 1;
    let ((name, name_span), header_idx) = parse_name_with_optional_hash(tokens, name_idx)?;
    let item_end = find_declaration_end(tokens, start_idx);
    let colon_idx =
        find_top_level_token(tokens, header_idx, item_end, |token| *token == Token::Colon)?;
    if colon_idx >= item_end {
        return None;
    }

    let externs_before =
        find_top_level_token(tokens, header_idx, colon_idx.saturating_sub(1), |token| {
            *token == Token::Equal
        })
        .and_then(|eq_idx| {
            (eq_idx < colon_idx.saturating_sub(1))
                .then(|| parse_extern_spec(tokens, eq_idx + 1, colon_idx.saturating_sub(1)))
        })
        .flatten();
    let post_sig_eq_idx = find_top_level_token(tokens, colon_idx + 1, item_end, |token| {
        *token == Token::Equal
    });
    let sig_end = post_sig_eq_idx.map_or(item_end, |idx| idx.saturating_sub(1));
    let signature = parse_type_expr(tokens, colon_idx + 1, sig_end);
    let externs_after = post_sig_eq_idx
        .and_then(|eq_idx| {
            (eq_idx < item_end).then(|| parse_extern_spec(tokens, eq_idx + 1, item_end))
        })
        .flatten();
    Some((
        (
            TopLevelDef::CallableSpec(CallableSpec {
                modifiers: DefModifiers::default(),
                kind,
                name: (name, name_span),
                externs: externs_before.or(externs_after),
                signature,
            }),
            span_for_indices(tokens, start_idx, item_end),
        ),
        item_end + 1,
    ))
}

fn parse_type_alias(
    tokens: &[(Token, Span)],
    start_idx: usize,
) -> Option<(Spanned<TopLevelDef>, usize)> {
    let name_idx = start_idx + 1;
    let eq_idx = find_top_level_token(
        tokens,
        name_idx + 1,
        tokens.len().saturating_sub(1),
        |token| *token == Token::Equal,
    );
    let item_end = eq_idx
        .map(|eq_idx| find_declaration_end(tokens, eq_idx))
        .unwrap_or_else(|| find_declaration_end(tokens, start_idx));
    let (name, header_end, is_operator) = match &tokens.get(name_idx)?.0 {
        Token::Id(keyword) if keyword == "operator" => {
            let op_start = name_idx + 1;
            let op_end =
                find_top_level_token_including_opens(tokens, op_start, item_end, |token| {
                    matches!(
                        token,
                        Token::LeftBracket | Token::Colon | Token::Equal | Token::RightArrow
                    )
                })
                .map_or(item_end, |idx| idx.saturating_sub(1));
            if op_start > op_end {
                return None;
            }
            (
                (
                    tokens_text(tokens, op_start, op_end),
                    span_for_indices(tokens, op_start, op_end),
                ),
                op_end,
                true,
            )
        }
        _ => {
            let name = token_as_ident(&tokens[name_idx].0)?;
            ((name, tokens[name_idx].1), name_idx, false)
        }
    };

    let eq_idx = eq_idx.filter(|eq_idx| *eq_idx <= item_end);
    let header_tail_end = eq_idx.map_or(item_end, |idx| idx.saturating_sub(1));
    let kind_arrow_idx =
        find_last_top_level_token(tokens, header_end + 1, header_tail_end, |token| {
            *token == Token::RightArrow
        })
        .filter(|arrow_idx| *arrow_idx < header_tail_end)
        .filter(|arrow_idx| kind_span(tokens, arrow_idx + 1, header_tail_end).is_some());
    let params = if header_end < header_tail_end && tokens[header_end + 1].0 == Token::LeftBracket {
        let params_end = kind_arrow_idx
            .map(|arrow_idx| arrow_idx.saturating_sub(1))
            .unwrap_or(header_tail_end);
        parse_type_param_spec(tokens, header_end + 1, params_end)
    } else {
        None
    };
    let kind = find_top_level_token(tokens, header_end + 1, header_tail_end, |token| {
        *token == Token::Colon
    })
    .and_then(|colon_idx| kind_span(tokens, colon_idx + 1, header_tail_end))
    .or_else(|| {
        kind_arrow_idx.and_then(|arrow_idx| kind_span(tokens, arrow_idx + 1, header_tail_end))
    });
    let target = eq_idx.and_then(|eq_idx| {
        (eq_idx < item_end).then(|| parse_type_expr(tokens, eq_idx + 1, item_end))
    });

    Some((
        (
            TopLevelDef::TypeAlias(TypeAliasDef {
                modifiers: DefModifiers::default(),
                name,
                params,
                kind,
                target,
                is_operator,
            }),
            span_for_indices(tokens, start_idx, item_end),
        ),
        item_end + 1,
    ))
}

fn parse_named_def(
    tokens: &[(Token, Span)],
    start_idx: usize,
    kind: NamedDefKind,
) -> Option<(Spanned<TopLevelDef>, usize)> {
    let name_idx = start_idx + 1;
    let name = match &tokens.get(name_idx)?.0 {
        Token::Id(name) => name.clone(),
        Token::Underscore if matches!(kind, NamedDefKind::Let | NamedDefKind::Var) => {
            "_".to_string()
        }
        _ => return None,
    };
    let item_end = find_declaration_end(tokens, start_idx);
    let eq_idx = find_top_level_token(tokens, name_idx + 1, item_end, |token| {
        *token == Token::Equal
    });
    let with_idx = matches!(kind, NamedDefKind::Enum)
        .then(|| {
            eq_idx.and_then(|eq_idx| {
                (name_idx + 1 < eq_idx).then(|| {
                    find_top_level_token(tokens, name_idx + 1, eq_idx.saturating_sub(1), |token| {
                        *token == Token::KwWith
                    })
                })?
            })
        })
        .flatten();
    let params = if matches!(kind, NamedDefKind::Struct | NamedDefKind::Union)
        && eq_idx.is_some()
        && name_idx < eq_idx.unwrap().saturating_sub(1)
        && tokens[name_idx + 1].0 == Token::LeftBracket
    {
        parse_type_param_spec(tokens, name_idx + 1, eq_idx.unwrap().saturating_sub(1))
    } else {
        None
    };
    let header_start = if params.is_some() {
        eq_idx.unwrap_or(name_idx + 1)
    } else {
        name_idx + 1
    };
    let ty = find_top_level_token(tokens, header_start, item_end, |token| {
        *token == Token::Colon
    })
    .and_then(|colon_idx| {
        let end = find_top_level_token(tokens, colon_idx + 1, item_end, |token| {
            *token == Token::Equal
        })
        .map_or(item_end, |idx| idx.saturating_sub(1));
        if colon_idx < end {
            Some(parse_type_expr(tokens, colon_idx + 1, end))
        } else {
            None
        }
    });
    let value_span = eq_idx.and_then(|eq_idx| {
        if eq_idx < item_end {
            Some(span_for_indices(tokens, eq_idx + 1, item_end))
        } else {
            None
        }
    });
    let detail = eq_idx
        .filter(|eq_idx| *eq_idx < item_end)
        .and_then(|eq_idx| match kind {
            NamedDefKind::Enum => {
                let members = parse_enum_member_list(tokens, eq_idx + 1, item_end);
                let functions = with_idx
                    .map(|with_idx| {
                        parse_enum_function_list(tokens, with_idx + 1, eq_idx.saturating_sub(1))
                    })
                    .unwrap_or_default();
                (!members.is_empty() || !functions.is_empty())
                    .then_some(NamedDefDetail::Enum { members, functions })
            }
            NamedDefKind::Struct => {
                let fields = parse_typed_field_block(tokens, eq_idx + 1, item_end);
                (!fields.is_empty()).then_some(NamedDefDetail::Struct { fields })
            }
            NamedDefKind::Union => {
                let variants = parse_union_variant_block(tokens, eq_idx + 1, item_end);
                (!variants.is_empty()).then_some(NamedDefDetail::Union { variants })
            }
            NamedDefKind::Bitfield => {
                let fields = parse_bitfield_field_block(tokens, eq_idx + 1, item_end);
                (!fields.is_empty()).then_some(NamedDefDetail::Bitfield { fields })
            }
            _ => None,
        });
    let members = if let Some(detail) = &detail {
        match detail {
            NamedDefDetail::Enum { members, .. } => {
                members.iter().map(|member| member.0.name.clone()).collect()
            }
            NamedDefDetail::Struct { fields } => {
                fields.iter().map(|field| field.0.name.clone()).collect()
            }
            NamedDefDetail::Union { variants } => variants
                .iter()
                .map(|variant| variant.0.name.clone())
                .collect(),
            NamedDefDetail::Bitfield { fields } => {
                fields.iter().map(|field| field.0.name.clone()).collect()
            }
        }
    } else if matches!(
        kind,
        NamedDefKind::Enum | NamedDefKind::Overload | NamedDefKind::Union
    ) {
        eq_idx
            .filter(|eq_idx| *eq_idx < item_end)
            .map(|eq_idx| parse_named_member_list(tokens, eq_idx + 1, item_end, kind))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let value = if matches!(
        kind,
        NamedDefKind::Register | NamedDefKind::Let | NamedDefKind::Var
    ) {
        eq_idx.and_then(|eq_idx| {
            (eq_idx < item_end).then(|| parse_expr(tokens, eq_idx + 1, item_end))
        })
    } else {
        None
    };

    Some((
        (
            TopLevelDef::Named(NamedDef {
                modifiers: DefModifiers::default(),
                kind,
                name: (name, tokens[name_idx].1),
                params,
                ty,
                members,
                detail,
                value,
                value_span,
            }),
            span_for_indices(tokens, start_idx, item_end),
        ),
        item_end + 1,
    ))
}

fn parse_scattered(
    tokens: &[(Token, Span)],
    start_idx: usize,
) -> Option<(Spanned<TopLevelDef>, usize)> {
    let kind_idx = start_idx + 1;
    let (kind, name_idx) = match tokens.get(kind_idx)?.0 {
        Token::KwFunction => (ScatteredKind::Function, kind_idx + 1),
        Token::KwMapping => (ScatteredKind::Mapping, kind_idx + 1),
        Token::KwUnion => (ScatteredKind::Union, kind_idx + 1),
        Token::KwEnum => (ScatteredKind::Enum, kind_idx + 1),
        _ => return None,
    };
    let item_end = find_declaration_end(tokens, name_idx);
    let Token::Id(name) = &tokens.get(name_idx)?.0 else {
        return None;
    };
    let params = if kind == ScatteredKind::Union {
        parse_type_param_spec(tokens, name_idx + 1, item_end)
    } else {
        None
    };
    let signature = if kind == ScatteredKind::Mapping {
        find_top_level_token(tokens, name_idx + 1, item_end, |token| {
            *token == Token::Colon
        })
        .and_then(|colon_idx| {
            (colon_idx < item_end).then(|| parse_type_expr(tokens, colon_idx + 1, item_end))
        })
    } else {
        None
    };
    Some((
        (
            TopLevelDef::Scattered(ScatteredDef {
                modifiers: DefModifiers::default(),
                kind,
                name: (name.clone(), tokens[name_idx].1),
                params,
                signature,
            }),
            span_for_indices(tokens, start_idx, item_end),
        ),
        item_end + 1,
    ))
}

fn parse_scattered_clause(
    tokens: &[(Token, Span)],
    start_idx: usize,
    kind: ScatteredClauseKind,
) -> Option<(Spanned<TopLevelDef>, usize)> {
    let item_end = find_declaration_end(tokens, start_idx);
    let clause_idx = start_idx + 1;
    if tokens.get(clause_idx).map(|(token, _)| token) != Some(&Token::KwClause) {
        return None;
    }
    let name_idx = clause_idx + 1;
    let member_idx = find_top_level_token(tokens, name_idx + 1, item_end, |token| {
        *token == Token::Equal
    })?
    .saturating_add(1);
    let name = token_as_ident(&tokens.get(name_idx)?.0)?;
    let member = token_as_ident(&tokens.get(member_idx)?.0)?;
    let ty = find_top_level_token(tokens, member_idx + 1, item_end, |token| {
        *token == Token::Colon
    })
    .and_then(|colon_idx| {
        (colon_idx < item_end).then(|| parse_type_expr(tokens, colon_idx + 1, item_end))
    });
    Some((
        (
            TopLevelDef::ScatteredClause(ScatteredClauseDef {
                modifiers: DefModifiers::default(),
                kind,
                name: (name, tokens[name_idx].1),
                member: (member, tokens[member_idx].1),
                ty,
            }),
            span_for_indices(tokens, start_idx, item_end),
        ),
        item_end + 1,
    ))
}

fn parse_default(
    tokens: &[(Token, Span)],
    start_idx: usize,
) -> Option<(Spanned<TopLevelDef>, usize)> {
    let item_end = find_declaration_end(tokens, start_idx);
    let kind_idx = start_idx + 1;
    let direction_idx = kind_idx + 1;
    let kind = token_as_ident(&tokens.get(kind_idx)?.0)?;
    let direction = token_as_ident(&tokens.get(direction_idx)?.0)?;
    Some((
        (
            TopLevelDef::Default(DefaultDef {
                modifiers: DefModifiers::default(),
                kind: (kind, tokens[kind_idx].1),
                direction: (direction, tokens[direction_idx].1),
            }),
            span_for_indices(tokens, start_idx, item_end),
        ),
        item_end + 1,
    ))
}

fn parse_fixity(
    tokens: &[(Token, Span)],
    start_idx: usize,
) -> Option<(Spanned<TopLevelDef>, usize)> {
    let item_end = find_declaration_end(tokens, start_idx);
    let kind = match tokens.get(start_idx)?.0 {
        Token::KwInfix => FixityKind::Infix,
        Token::KwInfixl => FixityKind::Infixl,
        Token::KwInfixr => FixityKind::Infixr,
        _ => return None,
    };
    let precedence_idx = start_idx + 1;
    let precedence_text = tokens_text(tokens, precedence_idx, precedence_idx);
    let operator_start = precedence_idx + 1;
    if operator_start > item_end {
        return None;
    }
    Some((
        (
            TopLevelDef::Fixity(FixityDef {
                modifiers: DefModifiers::default(),
                kind,
                precedence: (precedence_text, tokens[precedence_idx].1),
                operator: (
                    tokens_text(tokens, operator_start, item_end),
                    span_for_indices(tokens, operator_start, item_end),
                ),
            }),
            span_for_indices(tokens, start_idx, item_end),
        ),
        item_end + 1,
    ))
}

fn parse_instantiation(
    tokens: &[(Token, Span)],
    start_idx: usize,
) -> Option<(Spanned<TopLevelDef>, usize)> {
    let item_end = find_declaration_end(tokens, start_idx);
    let name_idx = start_idx + 1;
    let name = token_as_ident(&tokens.get(name_idx)?.0)?;
    let substitutions = find_top_level_token(tokens, name_idx + 1, item_end, |token| {
        *token == Token::KwWith
    })
    .map(|with_idx| {
        split_top_level_segments(tokens, with_idx + 1, item_end, |token| {
            *token == Token::Comma
        })
        .into_iter()
        .filter_map(|(segment_start, segment_end)| {
            let eq_idx = find_top_level_token(tokens, segment_start, segment_end, |token| {
                *token == Token::Equal
            })?;
            Some(InstantiationSubstitution {
                lhs: (
                    tokens_text(tokens, segment_start, eq_idx.saturating_sub(1)),
                    span_for_indices(tokens, segment_start, eq_idx.saturating_sub(1)),
                ),
                rhs: (
                    tokens_text(tokens, eq_idx + 1, segment_end),
                    span_for_indices(tokens, eq_idx + 1, segment_end),
                ),
            })
        })
        .collect()
    })
    .unwrap_or_default();
    Some((
        (
            TopLevelDef::Instantiation(InstantiationDef {
                modifiers: DefModifiers::default(),
                name: (name, tokens[name_idx].1),
                substitutions,
            }),
            span_for_indices(tokens, start_idx, item_end),
        ),
        item_end + 1,
    ))
}

fn parse_end(tokens: &[(Token, Span)], start_idx: usize) -> Option<(Spanned<TopLevelDef>, usize)> {
    let item_end = find_declaration_end(tokens, start_idx);
    let name_idx = start_idx + 1;
    let name = token_as_ident(&tokens.get(name_idx)?.0)?;
    Some((
        (
            TopLevelDef::End(EndDef {
                modifiers: DefModifiers::default(),
                name: (name, tokens[name_idx].1),
            }),
            span_for_indices(tokens, start_idx, item_end),
        ),
        item_end + 1,
    ))
}

fn parse_directive(
    tokens: &[(Token, Span)],
    start_idx: usize,
) -> Option<(Spanned<TopLevelDef>, usize)> {
    match tokens.get(start_idx)? {
        (Token::Directive { name, payload }, span) => Some((
            (
                TopLevelDef::Directive(DirectiveDef {
                    modifiers: DefModifiers::default(),
                    name: (name.clone(), *span),
                    payload: payload.as_ref().map(|payload| (payload.clone(), *span)),
                    structured_payload: None,
                }),
                *span,
            ),
            start_idx + 1,
        )),
        (Token::StructuredDirectiveStart(name), span) => {
            let end_idx = find_structured_directive_end(tokens, start_idx)?;
            let payload_span = if start_idx + 1 < end_idx {
                span_for_indices(tokens, start_idx + 1, end_idx - 1)
            } else {
                Span::new(span.end, tokens[end_idx].1.start)
            };
            let entries = if start_idx + 1 < end_idx {
                split_top_level_segments(tokens, start_idx + 1, end_idx - 1, |token| {
                    *token == Token::Comma
                })
                .into_iter()
                .map(|(segment_start, segment_end)| {
                    parse_attribute_entry(tokens, segment_start, segment_end)
                })
                .collect::<Option<Vec<_>>>()?
            } else {
                Vec::new()
            };
            Some((
                (
                    TopLevelDef::Directive(DirectiveDef {
                        modifiers: DefModifiers::default(),
                        name: (name.clone(), *span),
                        payload: None,
                        structured_payload: Some((AttributeData::Object(entries), payload_span)),
                    }),
                    span_for_indices(tokens, start_idx, end_idx),
                ),
                end_idx + 1,
            ))
        }
        _ => None,
    }
}

fn parse_constraint_def(
    tokens: &[(Token, Span)],
    start_idx: usize,
    is_type_constraint: bool,
) -> Option<(Spanned<TopLevelDef>, usize)> {
    let item_end = find_declaration_end(tokens, start_idx + usize::from(is_type_constraint));
    let ty_start = if is_type_constraint {
        start_idx + 2
    } else {
        start_idx + 1
    };
    if ty_start > item_end {
        return None;
    }
    Some((
        (
            TopLevelDef::Constraint(ConstraintDef {
                modifiers: DefModifiers::default(),
                ty: parse_type_expr(tokens, ty_start, item_end),
                is_type_constraint,
            }),
            span_for_indices(tokens, start_idx, item_end),
        ),
        item_end + 1,
    ))
}

fn parse_loop_measure(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Option<Spanned<LoopMeasure>> {
    if start >= end {
        return None;
    }
    let span = span_for_indices(tokens, start, end);
    let expr = parse_expr(tokens, start + 1, end);
    let measure = match tokens[start].0 {
        Token::KwUntil => LoopMeasure::Until(expr),
        Token::KwRepeat => LoopMeasure::Repeat(expr),
        Token::KwWhile => LoopMeasure::While(expr),
        _ => return None,
    };
    Some((measure, span))
}

fn parse_termination_measure(
    tokens: &[(Token, Span)],
    start_idx: usize,
) -> Option<(Spanned<TopLevelDef>, usize)> {
    let item_end = find_declaration_end(tokens, start_idx);
    let name_idx = start_idx + 1;
    let name = token_as_ident(&tokens.get(name_idx)?.0)?;
    let eq_idx = find_top_level_token(tokens, name_idx + 1, item_end, |token| {
        *token == Token::Equal
    });
    let kind = if let Some(eq_idx) = eq_idx {
        if name_idx + 1 <= eq_idx.saturating_sub(1) && eq_idx < item_end {
            TerminationMeasureKind::Function {
                pattern: parse_pattern(tokens, name_idx + 1, eq_idx.saturating_sub(1)),
                body: parse_expr(tokens, eq_idx + 1, item_end),
            }
        } else {
            return None;
        }
    } else {
        let measures = split_top_level_segments(tokens, name_idx + 1, item_end, |token| {
            *token == Token::Comma
        })
        .into_iter()
        .filter_map(|(segment_start, segment_end)| {
            parse_loop_measure(tokens, segment_start, segment_end)
        })
        .collect::<Vec<_>>();
        if measures.is_empty() {
            return None;
        }
        TerminationMeasureKind::Loop { measures }
    };

    Some((
        (
            TopLevelDef::TerminationMeasure(TerminationMeasureDef {
                modifiers: DefModifiers::default(),
                name: (name, tokens[name_idx].1),
                kind,
            }),
            span_for_indices(tokens, start_idx, item_end),
        ),
        item_end + 1,
    ))
}

fn parse_source_tokens(tokens: &[(Token, Span)]) -> SourceFile {
    let mut items = Vec::new();
    let mut idx = 0usize;

    while idx < tokens.len() {
        let (modifiers, cursor) = parse_def_modifiers(tokens, idx);
        if matches!(
            tokens.get(cursor).map(|(token, _)| token),
            Some(Token::KwOutcome)
        ) {
            idx = skip_outcome_item(tokens, cursor);
            continue;
        }
        let parsed = match tokens.get(cursor).map(|(token, _)| token) {
            Some(Token::KwScattered) => parse_scattered(tokens, cursor),
            Some(Token::KwFunction) => {
                parse_callable_def(tokens, cursor, CallableDefKind::Function)
            }
            Some(Token::KwVal) => parse_callable_spec(tokens, cursor, CallableSpecKind::Value),
            Some(Token::KwMapping) => {
                let decl_end = find_declaration_end(tokens, cursor);
                let has_eq = if cursor + 1 <= decl_end {
                    find_top_level_token(tokens, cursor + 1, decl_end, |token| {
                        *token == Token::Equal
                    })
                    .is_some()
                } else {
                    false
                };
                if tokens.get(cursor + 1).map(|(token, _)| token) == Some(&Token::KwClause) {
                    parse_callable_def(tokens, cursor, CallableDefKind::Mapping)
                } else if has_eq {
                    parse_mapping_def(tokens, cursor)
                } else {
                    parse_callable_spec(tokens, cursor, CallableSpecKind::Mapping)
                }
            }
            Some(Token::KwType) => {
                if tokens.get(cursor + 1).map(|(token, _)| token) == Some(&Token::KwConstraint) {
                    parse_constraint_def(tokens, cursor, true)
                } else {
                    parse_type_alias(tokens, cursor)
                }
            }
            Some(Token::KwConstraint) => parse_constraint_def(tokens, cursor, false),
            Some(Token::KwDefault) => parse_default(tokens, cursor),
            Some(Token::KwInstantiation) => parse_instantiation(tokens, cursor),
            Some(Token::KwInfix) | Some(Token::KwInfixl) | Some(Token::KwInfixr) => {
                parse_fixity(tokens, cursor)
            }
            Some(Token::Directive { .. }) | Some(Token::StructuredDirectiveStart(_)) => {
                parse_directive(tokens, cursor)
            }
            Some(Token::KwTerminationMeasure) => parse_termination_measure(tokens, cursor),
            Some(Token::KwEnd) => parse_end(tokens, cursor),
            Some(Token::KwRegister) => parse_named_def(tokens, cursor, NamedDefKind::Register),
            Some(Token::KwOverload) => parse_named_def(tokens, cursor, NamedDefKind::Overload),
            Some(Token::KwStruct) => parse_named_def(tokens, cursor, NamedDefKind::Struct),
            Some(Token::KwUnion) => {
                if tokens.get(cursor + 1).map(|(token, _)| token) == Some(&Token::KwClause) {
                    parse_scattered_clause(tokens, cursor, ScatteredClauseKind::Union)
                } else {
                    parse_named_def(tokens, cursor, NamedDefKind::Union)
                }
            }
            Some(Token::KwBitfield) => parse_named_def(tokens, cursor, NamedDefKind::Bitfield),
            Some(Token::KwEnum) => {
                if tokens.get(cursor + 1).map(|(token, _)| token) == Some(&Token::KwClause) {
                    parse_scattered_clause(tokens, cursor, ScatteredClauseKind::Enum)
                } else {
                    parse_named_def(tokens, cursor, NamedDefKind::Enum)
                }
            }
            Some(Token::KwNewtype) => parse_named_def(tokens, cursor, NamedDefKind::Newtype),
            Some(Token::KwLet) => parse_named_def(tokens, cursor, NamedDefKind::Let),
            Some(Token::KwVar) => parse_named_def(tokens, cursor, NamedDefKind::Var),
            _ => None,
        };

        if let Some((mut item, next_idx)) = parsed {
            apply_modifiers(&mut item.0, modifiers);
            items.push(item);
            idx = next_idx;
        } else {
            idx = cursor.max(idx + 1);
        }
    }

    SourceFile { items }
}

#[cfg(test)]
pub fn source_file_parser<'a>() -> impl Parser<'a, Input<'a>, SourceFile, ParseErr<'a>> {
    custom(|inp| {
        let start = inp.offset();
        let tokens = inp.slice_from(start..);
        let ast = parse_source_tokens(tokens);
        while inp.peek().is_some() {
            inp.skip();
        }
        Ok(ast)
    })
}

pub fn core_source_file_parser<'a>(
) -> impl Parser<'a, Input<'a>, crate::core_ast::SourceFile, ParseErr<'a>> {
    custom(|inp| {
        let start = inp.offset();
        let tokens = inp.slice_from(start..);
        let ast = crate::core_ast::SourceFile::from(&parse_source_tokens(tokens));
        while inp.peek().is_some() {
            inp.skip();
        }
        Ok(ast)
    })
}

#[cfg(test)]
pub fn parse_source(tokens: &[(Token, Span)]) -> ParseResult<SourceFile, Rich<'_, Token, Span>> {
    source_file_parser().parse(tokens.spanned(eoi_span(tokens)))
}

pub fn parse_core_source(
    tokens: &[(Token, Span)],
) -> ParseResult<crate::core_ast::SourceFile, Rich<'_, Token, Span>> {
    core_source_file_parser().parse(tokens.spanned(eoi_span(tokens)))
}

pub fn parse_expr_fragment(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Option<crate::core_ast::Spanned<crate::core_ast::Expr>> {
    if tokens.is_empty() || start > end || end >= tokens.len() {
        return None;
    }
    Some(crate::core_ast::lower_expr_spanned(&parse_expr(
        tokens, start, end,
    )))
}

#[cfg(test)]
mod tests {
    use chumsky::Parser;

    use super::*;

    #[test]
    fn parses_typed_function_head() {
        let source = "function foo(x : bits(32), y) -> int = x";
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        let TopLevelDef::CallableDef(def) = &ast.items[0].0 else {
            panic!("expected callable def");
        };
        assert_eq!(def.kind, CallableDefKind::Function);
        assert_eq!(def.name.0, "foo");
        assert_eq!(def.params.len(), 2);
        assert!(matches!(def.return_ty, Some((TypeExpr::Named(ref name), _)) if name == "int"));
        assert!(matches!(
            &def.params[0].0,
            Pattern::Typed(inner, (_, _)) if matches!(&inner.0, Pattern::Ident(name) if name == "x")
        ));
    }

    #[test]
    fn parses_value_and_mapping_specs() {
        let source = "val add : (int, int) -> int\nmapping enc : bits(32) <-> string\n";
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        assert!(matches!(
            &ast.items[0].0,
            TopLevelDef::CallableSpec(CallableSpec {
                kind: CallableSpecKind::Value,
                ..
            })
        ));
        assert!(matches!(
            &ast.items[1].0,
            TopLevelDef::CallableSpec(CallableSpec {
                kind: CallableSpecKind::Mapping,
                ..
            })
        ));
    }

    #[test]
    fn parses_extern_specs_forall_and_effects() {
        let source = r#"
val write_ram = {lem: "write_ram", coq: "write_ram"} : forall 'n, 0 < 'n <= max_mem_access . (write_kind, xlenbits, atom('n), bits(8 * 'n), mem_meta) -> bool effect {wmv, wmvt}
val __TraceMemoryWrite : forall 'n 'm. (atom('n), bits('m), bits(8 * 'n)) -> unit
val trace_name : unit -> unit = pure "trace_name"
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        match &ast.items[0].0 {
            TopLevelDef::CallableSpec(CallableSpec {
                externs, signature, ..
            }) => {
                assert!(matches!(
                    externs,
                    Some((
                        ExternSpec::Bindings {
                            purity: None,
                            bindings,
                        },
                        _
                    )) if bindings.len() == 2
                        && matches!(bindings[0].0.name, Some((ref name, _)) if name == "lem")
                        && matches!(bindings[1].0.name, Some((ref name, _)) if name == "coq")
                ));
                match &signature.0 {
                    TypeExpr::Forall {
                        vars,
                        constraint,
                        body,
                    } => {
                        assert_eq!(
                            vars.iter()
                                .map(|var| var.name.0.as_str())
                                .collect::<Vec<_>>(),
                            vec!["'n"]
                        );
                        assert!(constraint.is_some());
                        assert!(matches!(
                            body.0,
                            TypeExpr::Effect {
                                ty: _,
                                ref effects
                            } if effects.iter().map(|effect| effect.0.as_str()).collect::<Vec<_>>() == vec!["wmv", "wmvt"]
                        ));
                    }
                    other => panic!("{other:#?}"),
                }
            }
            other => panic!("{other:#?}"),
        }

        match &ast.items[1].0 {
            TopLevelDef::CallableSpec(CallableSpec { signature, .. }) => match &signature.0 {
                TypeExpr::Forall { vars, body, .. } => {
                    assert_eq!(
                        vars.iter()
                            .map(|var| var.name.0.as_str())
                            .collect::<Vec<_>>(),
                        vec!["'n", "'m"]
                    );
                    assert!(matches!(body.0, TypeExpr::Arrow { .. }));
                }
                other => panic!("{other:#?}"),
            },
            other => panic!("{other:#?}"),
        }

        match &ast.items[2].0 {
            TopLevelDef::CallableSpec(CallableSpec {
                externs, signature, ..
            }) => {
                assert!(matches!(signature.0, TypeExpr::Arrow { .. }));
                assert!(matches!(
                    externs,
                    Some((
                        ExternSpec::String {
                            purity: Some(ExternPurity::Pure),
                            value,
                        },
                        _
                    )) if value.0 == "\"trace_name\""
                ));
            }
            other => panic!("{other:#?}"),
        }
    }

    #[test]
    fn parses_hashed_callables_empty_constructor_patterns_and_shift_ops() {
        let source = r#"
val write_tag# : forall 'addrsize, 'addrsize in {32, 64}. (int('addrsize), bits('addrsize), bool) -> unit
function write_tag#(_, addrsize, addr, value) = __write_tag#(addrsize, addr, value)
function f(xs, opt) = {
  let ys = xs << length(xs) in
  match opt {
    None() => ys,
    Some(v) => write_tag#(32, ys, v)
  }
}
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        assert!(matches!(
            &ast.items[0].0,
            TopLevelDef::CallableSpec(CallableSpec { name, .. }) if name.0 == "write_tag#"
        ));
        assert!(matches!(
            &ast.items[1].0,
            TopLevelDef::CallableDef(CallableDef { name, .. }) if name.0 == "write_tag#"
        ));
        let TopLevelDef::CallableDef(def) = &ast.items[2].0 else {
            panic!("expected callable def");
        };
        let Some((Expr::Block(items), _)) = &def.body else {
            panic!("expected block body");
        };
        let BlockItem::Expr((Expr::Let { body, .. }, _)) = &items[0].0 else {
            panic!("expected let expression");
        };
        let Expr::Match { cases, .. } = &body.0 else {
            panic!("expected match body");
        };
        assert!(matches!(
            &cases[0].0.pattern.0,
            Pattern::App { callee, args }
                if callee.0 == "None"
                    && matches!(args.as_slice(), [(Pattern::Literal(Literal::Unit), _)])
        ));
    }

    #[test]
    fn parses_forall_groups_and_type_operator_ranges() {
        let source = r#"
val deref : forall ('a : Type) 'n, 'n >= 0. register('a) -> bits('n)
infix 3 ...
type operator ... ('n : Int) ('m : Int) = range('n, 'm)
let x : 3 ... 5 = 4
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        let TopLevelDef::CallableSpec(CallableSpec { signature, .. }) = &ast.items[0].0 else {
            panic!("expected callable spec");
        };
        assert!(matches!(
            &signature.0,
            TypeExpr::Forall { vars, constraint: Some(_), body } if vars.len() == 2
                && vars[0].name.0 == "'a"
                && vars[1].name.0 == "'n"
                && matches!(body.0, TypeExpr::Arrow { .. })
        ));
        let let_item = ast
            .items
            .iter()
            .find_map(|(item, _)| match item {
                TopLevelDef::Named(NamedDef {
                    kind: NamedDefKind::Let,
                    ty: Some((TypeExpr::Infix { op, .. }, _)),
                    ..
                }) => Some(op),
                _ => None,
            })
            .expect("expected typed let");
        assert_eq!(let_item.0, "...");
    }

    #[test]
    fn parses_attribute_wrapped_exprs_patterns_and_cases() {
        let source = r#"
function f(x) = $[trace] match x {
  $[bind] y => y,
  $[case] ($[alt] z if $[guard] true => z)
}
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        let TopLevelDef::CallableDef(def) = &ast.items[0].0 else {
            panic!("expected callable def");
        };
        let Some((Expr::Attribute { attr, expr }, _)) = def.body.as_ref() else {
            panic!("expected attributed expr");
        };
        assert_eq!(attr.0.name.0, "trace");
        let Expr::Match { cases, .. } = &expr.0 else {
            panic!("expected match expr");
        };
        assert_eq!(cases.len(), 2);
        assert!(cases[0].0.attrs.is_empty());
        assert!(matches!(
            &cases[0].0.pattern.0,
            Pattern::Attribute { attr, pattern }
                if attr.0.name.0 == "bind"
                    && matches!(pattern.0, Pattern::Ident(ref name) if name == "y")
        ));
        assert_eq!(cases[1].0.attrs.len(), 1);
        assert_eq!(cases[1].0.attrs[0].0.name.0, "case");
        assert!(matches!(
            &cases[1].0.pattern.0,
            Pattern::Attribute { attr, pattern }
                if attr.0.name.0 == "alt"
                    && matches!(pattern.0, Pattern::Ident(ref name) if name == "z")
        ));
        assert!(matches!(
            &cases[1].0.guard,
            Some((Expr::Attribute { attr, .. }, _)) if attr.0.name.0 == "guard"
        ));
    }

    #[test]
    fn parses_top_level_modifiers_and_missing_defs() {
        let source = r#"
Private $[trace enabled] type child = parent
constraint bits(8)
type constraint atom('n)
termination_measure helper x = call(x)
termination_measure loop until done(x), while guard(x)
end helper
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        assert!(matches!(
            &ast.items[0].0,
            TopLevelDef::TypeAlias(TypeAliasDef { modifiers, name, .. })
                if modifiers.is_private
                    && modifiers.attrs.len() == 1
                    && modifiers.attrs[0].0.name.0 == "trace"
                    && modifiers.attrs[0].0.data.as_ref().map(|data| data.0.as_str()) == Some("enabled")
                    && matches!(modifiers.attrs[0].0.parsed_data, Some((AttributeData::Ident(ref value), _)) if value == "enabled")
                    && name.0 == "child"
        ));
        assert!(matches!(
            &ast.items[1].0,
            TopLevelDef::Constraint(ConstraintDef {
                is_type_constraint: false,
                ..
            })
        ));
        assert!(matches!(
            &ast.items[2].0,
            TopLevelDef::Constraint(ConstraintDef {
                is_type_constraint: true,
                ..
            })
        ));
        assert!(matches!(
            &ast.items[3].0,
            TopLevelDef::TerminationMeasure(TerminationMeasureDef {
                name,
                kind: TerminationMeasureKind::Function { .. },
                ..
            }) if name.0 == "helper"
        ));
        assert!(matches!(
            &ast.items[4].0,
            TopLevelDef::TerminationMeasure(TerminationMeasureDef {
                name,
                kind: TerminationMeasureKind::Loop { measures },
                ..
            }) if name.0 == "loop" && measures.len() == 2
        ));
        assert!(matches!(
            &ast.items[5].0,
            TopLevelDef::End(EndDef { name, .. }) if name.0 == "helper"
        ));
    }

    #[test]
    fn parses_lowercase_private_modifiers() {
        let source = r#"
private function helper(x) = {
  x
}
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        assert!(matches!(
            &ast.items[0].0,
            TopLevelDef::CallableDef(CallableDef { modifiers, name, .. })
                if modifiers.is_private && name.0 == "helper"
        ));
    }

    #[test]
    fn parses_type_params_and_kind_annotations() {
        let source = r#"
type vec('n) -> Type = bits('n)
type bounded(('n : Int)) constraint 0 < 'n = bits('n)
struct pair('a, 'b) = { fst : 'a, snd : 'b }
union opt('a) = { None : unit, Some : 'a }
scattered union tree('a)
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        assert!(matches!(
            &ast.items[0].0,
            TopLevelDef::TypeAlias(TypeAliasDef { params: Some((params, _)), kind: Some((kind, _)), .. })
                if params.params.len() == 1 && params.params[0].name.0 == "'n" && kind == "Type"
        ));
        assert!(matches!(
            &ast.items[1].0,
            TopLevelDef::TypeAlias(TypeAliasDef { params: Some((params, _)), .. })
                if matches!(params.tail, Some(TypeParamTail::Constraint(_)))
                    && params.params.len() == 1
                    && params.params[0].kind.as_ref().map(|kind| kind.0.as_str()) == Some("Int")
        ));
        assert!(matches!(
            &ast.items[2].0,
            TopLevelDef::Named(NamedDef { params: Some((params, _)), kind: NamedDefKind::Struct, .. })
                if params.params.len() == 2
        ));
        assert!(matches!(
            &ast.items[3].0,
            TopLevelDef::Named(NamedDef { params: Some((params, _)), kind: NamedDefKind::Union, .. })
                if params.params.len() == 1 && params.params[0].name.0 == "'a"
        ));
        assert!(matches!(
            &ast.items[4].0,
            TopLevelDef::Scattered(ScatteredDef { params: Some((params, _)), kind: ScatteredKind::Union, .. })
                if params.params.len() == 1 && params.params[0].name.0 == "'a"
        ));
    }

    #[test]
    fn parses_mapping_clause_body() {
        let source = r#"mapping clause assembly = use_bits(0x12) <-> "ok""#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        let TopLevelDef::CallableDef(def) = &ast.items[0].0 else {
            panic!("expected callable def");
        };
        assert_eq!(def.kind, CallableDefKind::MappingClause);
        let body = def.mapping_body.as_ref().expect("mapping body");
        assert_eq!(body.arms.len(), 1);
        assert!(matches!(body.arms[0].0.lhs.0, Expr::Call(_)));
        assert!(matches!(
            body.arms[0].0.rhs.0,
            Expr::Literal(Literal::String(_))
        ));
    }

    #[test]
    fn parses_mapping_defs_with_signature_directions_and_guards() {
        let source = r#"
mapping size_bits : word_width <-> bits(2) = {
  BYTE <-> 0b00,
  forwards DOUBLE => 0b11,
  backwards bits if allow(bits) => decode(bits)
}

mapping clause assembly =
  instr <-> decode(instr)
when
  valid(instr)
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        let TopLevelDef::CallableDef(def) = &ast.items[0].0 else {
            panic!("expected mapping def");
        };
        assert_eq!(def.kind, CallableDefKind::Mapping);
        assert!(matches!(
            def.signature,
            Some((
                TypeExpr::Arrow {
                    kind: TypeArrowKind::Mapping,
                    ..
                },
                _
            ))
        ));
        let body = def.mapping_body.as_ref().expect("mapping body");
        assert_eq!(body.arms.len(), 3);
        assert!(matches!(
            body.arms[0].0.direction,
            MappingArmDirection::Bidirectional
        ));
        assert!(matches!(
            body.arms[1].0.direction,
            MappingArmDirection::Forwards
        ));
        assert!(matches!(
            body.arms[2].0.direction,
            MappingArmDirection::Backwards
        ));
        assert!(matches!(body.arms[2].0.guard, Some((Expr::Call(_), _))));
        assert!(matches!(
            body.arms[2].0.lhs_pattern,
            Some((Pattern::Ident(ref name), _)) if name == "bits"
        ));

        let TopLevelDef::CallableDef(clause) = &ast.items[1].0 else {
            panic!("expected mapping clause");
        };
        let clause_body = clause.mapping_body.as_ref().expect("mapping clause body");
        assert!(matches!(
            clause_body.arms[0].0.guard,
            Some((Expr::Call(_), _))
        ));
    }

    #[test]
    fn parses_assert_expr() {
        let source = r#"
function f(x) = {
  assert(x == 0x12, "bad x");
  assert(true)
}
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        let TopLevelDef::CallableDef(def) = &ast.items[0].0 else {
            panic!("expected callable def");
        };
        let Some((Expr::Block(items), _)) = &def.body else {
            panic!("expected block body");
        };
        assert!(matches!(
            &items[0].0,
            BlockItem::Expr((
                Expr::Assert {
                    condition,
                    message: Some(message),
                },
                _
            )) if matches!(condition.0, Expr::Infix { .. })
                && matches!(message.0, Expr::Literal(Literal::String(_)))
        ));
        assert!(matches!(
            &items[1].0,
            BlockItem::Expr((
                Expr::Assert {
                    condition,
                    message: None,
                },
                _
            )) if matches!(condition.0, Expr::Literal(Literal::Bool(true)))
        ));
    }

    #[test]
    fn parses_atomic_type_markers() {
        let source = r#"
type any = _
type down = dec
type up = inc
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        assert!(matches!(
            &ast.items[0].0,
            TopLevelDef::TypeAlias(TypeAliasDef {
                target: Some((TypeExpr::Wild, _)),
                ..
            })
        ));
        assert!(matches!(
            &ast.items[1].0,
            TopLevelDef::TypeAlias(TypeAliasDef {
                target: Some((TypeExpr::Dec, _)),
                ..
            })
        ));
        assert!(matches!(
            &ast.items[2].0,
            TopLevelDef::TypeAlias(TypeAliasDef {
                target: Some((TypeExpr::Inc, _)),
                ..
            })
        ));
    }

    #[test]
    fn parses_scattered_and_type_alias() {
        let source = "scattered function foo\ntype child = parent\n";
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = source_file_parser()
            .parse(tokens.as_slice().spanned(eoi_span(&tokens)))
            .into_result()
            .unwrap();

        assert!(matches!(
            &ast.items[0].0,
            TopLevelDef::Scattered(ScatteredDef {
                kind: ScatteredKind::Function,
                name,
                ..
            }) if name.0 == "foo"
        ));
        assert!(matches!(
            &ast.items[1].0,
            TopLevelDef::TypeAlias(TypeAliasDef { name, .. }) if name.0 == "child"
        ));
    }

    #[test]
    fn parses_defaults_fixity_instantiation_and_scattered_clauses() {
        let source = r#"
default Order dec
infixl 7 <<
instantiation sail_barrier with 'barrier = barrier_kind
enum clause extension = Ext_M
union clause instruction = ADD : bits(32)
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        assert!(matches!(
            &ast.items[0].0,
            TopLevelDef::Default(DefaultDef { kind, direction, .. })
                if kind.0 == "Order" && direction.0 == "dec"
        ));
        assert!(matches!(
            &ast.items[1].0,
            TopLevelDef::Fixity(FixityDef { kind: FixityKind::Infixl, precedence, operator, .. })
                if precedence.0 == "7" && operator.0 == "<<"
        ));
        match &ast.items[2].0 {
            TopLevelDef::Instantiation(InstantiationDef {
                name,
                substitutions,
                ..
            }) => {
                assert_eq!(name.0, "sail_barrier");
                assert_eq!(substitutions.len(), 1, "{substitutions:#?}");
                assert_eq!(substitutions[0].lhs.0, "'barrier");
                assert_eq!(substitutions[0].rhs.0, "barrier_kind");
            }
            other => panic!("{other:#?}"),
        }
        assert!(matches!(
            &ast.items[3].0,
            TopLevelDef::ScatteredClause(ScatteredClauseDef {
                kind: ScatteredClauseKind::Enum,
                name,
                member,
                ty: None,
                ..
            }) if name.0 == "extension" && member.0 == "Ext_M"
        ));
        assert!(matches!(
            &ast.items[4].0,
            TopLevelDef::ScatteredClause(ScatteredClauseDef {
                kind: ScatteredClauseKind::Union,
                name,
                member,
                ty: Some((TypeExpr::App { callee, .. }, _)),
                ..
            }) if name.0 == "instruction" && member.0 == "ADD" && callee.0 == "bits"
        ));
    }

    #[test]
    fn parses_richer_type_forms() {
        let source = r#"
type xlen : Int = config base.xlen
type log2_xlen : Int = if xlen == 32 then 5 else 6
type bounded = { ('n : Int), 'n >= 0 . bits('n) }
type operator ... ('n : Int) ('m : Int) = range('n, 'm)
enum color with enc -> bits(2), show -> string = { Red => 0b00, Green }
union instruction = { ADD : bits(32), SUB : bits(32) }
scattered mapping enc : bits(32) <-> string
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        assert!(matches!(
            &ast.items[0].0,
            TopLevelDef::TypeAlias(TypeAliasDef {
                name,
                kind: Some((kind, _)),
                target: Some((TypeExpr::Config(parts), _)),
                is_operator: false,
                ..
            }) if name.0 == "xlen" && kind == "Int" && parts.iter().map(|part| part.0.as_str()).collect::<Vec<_>>() == vec!["base", "xlen"]
        ));
        assert!(matches!(
            &ast.items[1].0,
            TopLevelDef::TypeAlias(TypeAliasDef {
                target: Some((TypeExpr::Conditional { .. }, _)),
                ..
            })
        ));
        assert!(matches!(
            &ast.items[2].0,
            TopLevelDef::TypeAlias(TypeAliasDef {
                target: Some((
                    TypeExpr::Existential {
                        binder,
                        constraint: Some(_),
                        body,
                    },
                    _,
                )),
                ..
            }) if binder.name.0 == "'n"
                && binder.kind.as_ref().map(|kind| kind.0.as_str()) == Some("Int")
                && matches!(body.0, TypeExpr::App { ref callee, .. } if callee.0 == "bits")
        ));
        assert!(matches!(
            &ast.items[3].0,
            TopLevelDef::TypeAlias(TypeAliasDef {
                name,
                is_operator: true,
                target: Some((TypeExpr::App { callee, .. }, _)),
                ..
            }) if name.0 == "..." && callee.0 == "range"
        ));
        assert!(matches!(
            &ast.items[4].0,
            TopLevelDef::Named(NamedDef {
                kind: NamedDefKind::Enum,
                members,
                detail: Some(NamedDefDetail::Enum { members: enum_members, functions }),
                ..
            }) if members.iter().map(|member| member.0.as_str()).collect::<Vec<_>>() == vec!["Red", "Green"]
                && enum_members.len() == 2
                && functions.iter().map(|function| function.0.name.0.as_str()).collect::<Vec<_>>() == vec!["enc", "show"]
                && matches!(enum_members[0].0.value, Some((Expr::Literal(Literal::Binary(ref bits)), _)) if bits == "0b00")
        ));
        assert!(matches!(
            &ast.items[5].0,
            TopLevelDef::Named(NamedDef {
                kind: NamedDefKind::Union,
                members,
                ..
            }) if members.iter().map(|member| member.0.as_str()).collect::<Vec<_>>() == vec!["ADD", "SUB"]
        ));
        match &ast.items[6].0 {
            TopLevelDef::Scattered(ScatteredDef {
                kind: ScatteredKind::Mapping,
                signature,
                ..
            }) => {
                assert!(
                    matches!(
                        signature,
                        Some((
                            TypeExpr::Arrow {
                                kind: TypeArrowKind::Mapping,
                                ..
                            },
                            _
                        ))
                    ),
                    "{signature:#?}"
                );
            }
            other => panic!("{other:#?}"),
        }
    }

    #[test]
    fn parses_struct_union_bitfield_details_and_as_patterns() {
        let source = r#"
struct S = { field1 : int, field2 : bits(3) }
union U = { Foo : int, Bar : { x : int, y : int } }
bitfield B : bits(8) = { HI : 7 .. 4, LO : 3 }
function f(xs) = match xs {
  x :: xs as zs => zs,
  flag[3] => xs,
  flag[7 .. 4] => zs
}
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        assert!(matches!(
            &ast.items[0].0,
            TopLevelDef::Named(NamedDef {
                kind: NamedDefKind::Struct,
                detail: Some(NamedDefDetail::Struct { fields }),
                ..
            }) if fields.len() == 2 && fields[0].0.name.0 == "field1"
        ));
        assert!(matches!(
            &ast.items[1].0,
            TopLevelDef::Named(NamedDef {
                kind: NamedDefKind::Union,
                detail: Some(NamedDefDetail::Union { variants }),
                ..
            }) if variants.len() == 2
                && variants[0].0.name.0 == "Foo"
                && matches!(variants[1].0.payload, UnionPayload::Struct { .. })
        ));
        assert!(matches!(
            &ast.items[2].0,
            TopLevelDef::Named(NamedDef {
                kind: NamedDefKind::Bitfield,
                detail: Some(NamedDefDetail::Bitfield { fields }),
                ..
            }) if fields.len() == 2
                && fields[0].0.name.0 == "HI"
                && fields[0].0.low.is_some()
                && fields[1].0.low.is_none()
        ));

        let TopLevelDef::CallableDef(def) = &ast.items[3].0 else {
            panic!("expected callable def");
        };
        let Some((Expr::Match { cases, .. }, _)) = &def.body else {
            panic!("expected match body");
        };
        assert!(matches!(
            &cases[0].0.pattern.0,
            Pattern::AsBinding { binding, .. } if binding.0 == "zs"
        ));
        assert!(matches!(
            &cases[1].0.pattern.0,
            Pattern::Index { name, .. } if name.0 == "flag"
        ));
        assert!(matches!(
            &cases[2].0.pattern.0,
            Pattern::RangeIndex { name, .. } if name.0 == "flag"
        ));
    }

    #[test]
    fn parses_union_variants_without_commas() {
        let source = r#"
union write_kind = {
  Write_plain : unit
  Write_release : unit
  Write_exclusive : unit
}
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();
        assert!(matches!(
            &ast.items[0].0,
            TopLevelDef::Named(NamedDef {
                kind: NamedDefKind::Union,
                detail: Some(NamedDefDetail::Union { variants }),
                ..
            }) if variants.len() == 3
                && variants[0].0.name.0 == "Write_plain"
                && variants[1].0.name.0 == "Write_release"
                && variants[2].0.name.0 == "Write_exclusive"
        ));
    }

    #[test]
    fn parses_block_and_control_flow_bodies() {
        let source = r#"
function foo(x) = {
  let y : int = bar(x);
  if x == y then return y else baz(y)
}
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        let TopLevelDef::CallableDef(def) = &ast.items[0].0 else {
            panic!("expected callable def");
        };
        let Some((Expr::Block(items), _)) = &def.body else {
            panic!("expected block body");
        };
        assert_eq!(items.len(), 2);
        assert!(matches!(
            &items[0].0,
            BlockItem::Let(LetBinding { pattern, .. })
                if matches!(&pattern.0, Pattern::Typed(inner, _) if matches!(&inner.0, Pattern::Ident(name) if name == "y"))
        ));
        assert!(matches!(&items[1].0, BlockItem::Expr((Expr::If { .. }, _))));
    }

    #[test]
    fn parses_nested_if_else_chains_with_dangling_else() {
        let source = r#"
function foo(a, b) =
  if a then if b then 1 else 2 else if b then 3 else 4
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        let TopLevelDef::CallableDef(def) = &ast.items[0].0 else {
            panic!("expected callable def");
        };
        let Some((
            Expr::If {
                then_branch,
                else_branch,
                ..
            },
            _,
        )) = &def.body
        else {
            panic!("expected outer if");
        };
        assert!(matches!(then_branch.0, Expr::If { .. }));
        assert!(matches!(
            else_branch,
            Some(branch) if matches!(branch.0, Expr::If { .. })
        ));
    }

    #[test]
    fn parses_match_and_vector_updates() {
        let source = r#"
function foo(r, v) = {
  var res = [v with 0 = bar(r), 1 .. 2 = baz()];
  match r {
    Some(x) => x,
    None() => 0
  }
}
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        let TopLevelDef::CallableDef(def) = &ast.items[0].0 else {
            panic!("expected callable def");
        };
        let Some((Expr::Block(items), _)) = &def.body else {
            panic!("expected block body");
        };
        assert!(matches!(
            &items[0].0,
            BlockItem::Var { value: (Expr::Call(call), _), .. }
                if matches!(&call.callee.0, Expr::Ident(name) if name == "vector_update_subrange#")
        ));
        assert!(matches!(
            &items[1].0,
            BlockItem::Expr((Expr::Match { cases, .. }, _)) if cases.len() == 2
        ));
    }

    #[test]
    fn parses_official_list_and_vector_syntax() {
        let source = r#"
function f(xs) = {
  let v = [1, 2];
  let l = [|1, 2|];
  match xs {
    [x, y] => v,
    [|a, b|] => l
  }
}
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        let TopLevelDef::CallableDef(def) = &ast.items[0].0 else {
            panic!("expected callable def");
        };
        let Some((Expr::Block(items), _)) = &def.body else {
            panic!("expected block body");
        };

        assert!(matches!(
            &items[0].0,
            BlockItem::Let(LetBinding { value, .. })
                if matches!(value.as_ref().0, Expr::Vector(ref items) if items.len() == 2)
        ));
        assert!(matches!(
            &items[1].0,
            BlockItem::Let(LetBinding { value, .. })
                if matches!(value.as_ref().0, Expr::List(ref items) if items.len() == 2)
        ));

        let BlockItem::Expr((Expr::Match { cases, .. }, _)) = &items[2].0 else {
            panic!("expected match expression");
        };
        assert!(matches!(&cases[0].0.pattern.0, Pattern::Vector(items) if items.len() == 2));
        assert!(matches!(&cases[1].0.pattern.0, Pattern::List(items) if items.len() == 2));
    }

    #[test]
    fn rewrites_official_modifier_call_sugar_in_parser() {
        let source = r#"
function dot(x, y) = x.foo(y)
function arrow(r) = r->bar()
function field(x) = x.foo
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        let TopLevelDef::CallableDef(dot_def) = &ast.items[0].0 else {
            panic!("expected dot callable def");
        };
        let Some((Expr::Call(call), _)) = &dot_def.body else {
            panic!("expected dot modifier call");
        };
        assert!(matches!(&call.callee.0, Expr::Ident(name) if name == "_mod_foo"));
        assert!(matches!(&call.args[0].0, Expr::Ident(name) if name == "x"));
        assert!(matches!(&call.args[1].0, Expr::Ident(name) if name == "y"));

        let TopLevelDef::CallableDef(arrow_def) = &ast.items[1].0 else {
            panic!("expected arrow callable def");
        };
        let Some((Expr::Call(call), _)) = &arrow_def.body else {
            panic!("expected arrow modifier call");
        };
        assert!(matches!(&call.callee.0, Expr::Ident(name) if name == "_mod_bar"));
        assert!(matches!(&call.args[0].0, Expr::Ref((name, _)) if name == "r"));
        assert_eq!(call.args.len(), 1);

        let TopLevelDef::CallableDef(field_def) = &ast.items[2].0 else {
            panic!("expected field callable def");
        };
        assert!(matches!(
            &field_def.body,
            Some((Expr::Field { field, via_arrow, .. }, _))
                if field.0 == "foo" && !via_arrow
        ));
    }

    #[test]
    fn rewrites_unit_calls_and_patterns_like_official_parser() {
        let source = r#"
function use(foo) = foo()
function match_none(xs) = match xs { None() => 0 }
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        let TopLevelDef::CallableDef(use_def) = &ast.items[0].0 else {
            panic!("expected use callable def");
        };
        let Some((Expr::Call(call), _)) = &use_def.body else {
            panic!("expected unit call");
        };
        assert!(matches!(&call.callee.0, Expr::Ident(name) if name == "foo"));
        assert!(matches!(
            call.args.as_slice(),
            [(Expr::Literal(Literal::Unit), _)]
        ));

        let TopLevelDef::CallableDef(match_def) = &ast.items[1].0 else {
            panic!("expected match callable def");
        };
        let Some((Expr::Match { cases, .. }, _)) = &match_def.body else {
            panic!("expected match body");
        };
        assert!(matches!(
            &cases[0].0.pattern.0,
            Pattern::App { callee, args }
                if callee.0 == "None"
                    && matches!(args.as_slice(), [(Pattern::Literal(Literal::Unit), _)])
        ));
    }

    #[test]
    fn rewrites_comma_slice_sugar_in_parser() {
        let source = "function mid(x) = x[2, 3]\n";
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        let TopLevelDef::CallableDef(def) = &ast.items[0].0 else {
            panic!("expected callable def");
        };
        let Some((Expr::Call(call), _)) = &def.body else {
            panic!("expected slice call");
        };
        assert!(matches!(&call.callee.0, Expr::Ident(name) if name == "slice"));
        assert!(matches!(&call.args[0].0, Expr::Ident(name) if name == "x"));
        assert!(matches!(&call.args[1].0, Expr::Literal(Literal::Number(ref n)) if n == "2"));
        assert!(matches!(&call.args[2].0, Expr::Literal(Literal::Number(ref n)) if n == "3"));
        assert_eq!(call.arg_separator_spans.len(), 1);
    }

    #[test]
    fn rewrites_vector_access_and_subrange_into_builtin_calls() {
        let source = "function proj(x) = (x[3], x[7 .. 4])\n";
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        let TopLevelDef::CallableDef(def) = &ast.items[0].0 else {
            panic!("expected callable def");
        };
        let Some((Expr::Tuple(items), _)) = &def.body else {
            panic!("expected tuple body");
        };

        let Expr::Call(access) = &items[0].0 else {
            panic!("expected vector access call");
        };
        assert!(matches!(
            &access.callee.0,
            Expr::Ident(name) if name == "vector_access#"
        ));
        assert!(matches!(&access.args[0].0, Expr::Ident(name) if name == "x"));
        assert!(matches!(&access.args[1].0, Expr::Literal(Literal::Number(ref n)) if n == "3"));

        let Expr::Call(subrange) = &items[1].0 else {
            panic!("expected vector subrange call");
        };
        assert!(matches!(
            &subrange.callee.0,
            Expr::Ident(name) if name == "vector_subrange#"
        ));
        assert!(matches!(&subrange.args[0].0, Expr::Ident(name) if name == "x"));
        assert!(matches!(
            &subrange.args[1].0,
            Expr::Literal(Literal::Number(ref n)) if n == "7"
        ));
        assert!(matches!(
            &subrange.args[2].0,
            Expr::Literal(Literal::Number(ref n)) if n == "4"
        ));
    }

    #[test]
    fn rewrites_vector_updates_into_nested_builtin_calls() {
        let source = "function patch(x, y, z) = [x with 0 = y, 7 .. 4 = z]\n";
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        let TopLevelDef::CallableDef(def) = &ast.items[0].0 else {
            panic!("expected callable def");
        };
        let Some((Expr::Call(outer), _)) = &def.body else {
            panic!("expected outer vector update call");
        };
        assert!(matches!(
            &outer.callee.0,
            Expr::Ident(name) if name == "vector_update_subrange#"
        ));
        assert!(matches!(&outer.args[1].0, Expr::Literal(Literal::Number(ref n)) if n == "7"));
        assert!(matches!(&outer.args[2].0, Expr::Literal(Literal::Number(ref n)) if n == "4"));
        assert!(matches!(&outer.args[3].0, Expr::Ident(name) if name == "z"));

        let Expr::Call(inner) = &outer.args[0].0 else {
            panic!("expected nested vector update call");
        };
        assert!(matches!(
            &inner.callee.0,
            Expr::Ident(name) if name == "vector_update#"
        ));
        assert!(matches!(&inner.args[0].0, Expr::Ident(name) if name == "x"));
        assert!(matches!(&inner.args[1].0, Expr::Literal(Literal::Number(ref n)) if n == "0"));
        assert!(matches!(&inner.args[2].0, Expr::Ident(name) if name == "y"));
    }

    #[test]
    fn parses_legacy_switch_colon_equal_tilde_and_update_operands() {
        let source = r#"
function legacy(s, opt, x) = {
  y := ~(x);
  assert(~(s == { s with field1 = 0 }), "bad");
  switch opt {
    case Some(v) -> { y := v; y }
    case None() -> 0
  }
}
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        let TopLevelDef::CallableDef(def) = &ast.items[0].0 else {
            panic!("expected callable def");
        };
        let Some((Expr::Block(items), _)) = &def.body else {
            panic!("expected block body");
        };
        assert!(matches!(
            &items[0].0,
            BlockItem::Expr((
                Expr::Assign {
                    lhs,
                    rhs,
                },
                _
            )) if matches!(lhs.0, Expr::Ident(ref name) if name == "y")
                && matches!(rhs.0, Expr::Prefix { ref op, .. } if op.0 == "~")
        ));
        assert!(matches!(
            &items[1].0,
            BlockItem::Expr((
                Expr::Assert {
                    condition,
                    message: Some(_),
                },
                _
            )) if matches!(condition.0, Expr::Prefix { .. })
        ));
        assert!(matches!(
            &items[2].0,
            BlockItem::Expr((Expr::Match { cases, .. }, _)) if cases.len() == 2
        ));
    }

    #[test]
    fn parses_config_expr_casts_and_composite_infix_ops() {
        let source = r#"
function f(x, y) = {
  let ok : bool = (config memory.pmp.na4_supported : bool);
  if x <_u y then x <<< 3 else y >>> 1
}
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        let TopLevelDef::CallableDef(def) = &ast.items[0].0 else {
            panic!("expected callable def");
        };
        let Some((Expr::Block(items), _)) = &def.body else {
            panic!("expected block body");
        };
        assert!(matches!(
            &items[0].0,
            BlockItem::Let(LetBinding { value, .. })
                if matches!(&value.0, Expr::Cast { expr, .. } if matches!(expr.0, Expr::Config(_)))
        ));
        assert!(matches!(
            &items[1].0,
            BlockItem::Expr((
                Expr::If {
                    cond,
                    then_branch,
                    else_branch: Some(else_branch),
                },
                _
            )) if matches!(cond.0, Expr::Infix { ref op, .. } if op.0 == "<_u")
                && matches!(then_branch.0, Expr::Infix { ref op, .. } if op.0 == "<<<")
                && matches!(else_branch.0, Expr::Infix { ref op, .. } if op.0 == ">>>")
        ));
    }

    #[test]
    fn parses_implication_operator() {
        let source = r#"
function f(a, b, c) =
  (a & b) ==> not(c)
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        let TopLevelDef::CallableDef(def) = &ast.items[0].0 else {
            panic!("expected callable def");
        };
        let Some((Expr::Infix { op, .. }, _)) = &def.body else {
            panic!("expected implication expr");
        };
        assert_eq!(op.0, "==>");
    }

    #[test]
    fn parses_suffixed_comparison_ops_and_operator_defs() {
        let source = r#"
function operator <=_u (x, y) = unsigned(x) <= unsigned(y)
function f(x, y) =
  if x <=_u y then x else y
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        assert!(matches!(
            &ast.items[0].0,
            TopLevelDef::CallableDef(CallableDef { name, .. }) if name.0 == "<=_u"
        ));
        let TopLevelDef::CallableDef(def) = &ast.items[1].0 else {
            panic!("expected callable def");
        };
        let Some((Expr::If { cond, .. }, _)) = &def.body else {
            panic!("expected if expr");
        };
        assert!(matches!(cond.0, Expr::Infix { ref op, .. } if op.0 == "<=_u"));
    }

    #[test]
    fn parses_exit_expr() {
        let source = "function foo() = exit 1";
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        let TopLevelDef::CallableDef(def) = &ast.items[0].0 else {
            panic!("expected callable def");
        };
        let Some((Expr::Exit(Some(expr)), _)) = &def.body else {
            panic!("expected exit expr");
        };
        assert!(matches!(expr.0, Expr::Literal(Literal::Number(_))));
    }

    #[test]
    fn parses_foreach_and_slice_assignment() {
        let source = r#"
function to_bytes_le(n, b) = {
  var res = vector_init(n, sail_zeros(8));
  foreach (i from 0 to (n - 1)) {
    res[i] = b[(8 * i + 7) .. (8 * i)]
  };
  res
}
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        let TopLevelDef::CallableDef(def) = &ast.items[0].0 else {
            panic!("expected callable def");
        };
        let Some((Expr::Block(items), _)) = &def.body else {
            panic!("expected block body");
        };
        let BlockItem::Expr((Expr::Foreach(foreach), _)) = &items[1].0 else {
            panic!("{:#?}", items[1]);
        };
        assert_eq!(foreach.iterator.0, "i");
        assert_eq!(foreach.start_keyword.0, "from");
        assert_eq!(foreach.end_keyword.0, "to");
        assert!(matches!(
            foreach.start.0,
            Expr::Literal(Literal::Number(ref value)) if value == "0"
        ));
        assert!(matches!(foreach.end.0, Expr::Tuple(_) | Expr::Infix { .. }));
        assert!(foreach.step.is_none());
        assert!(foreach.ty.is_none());
    }

    #[test]
    fn parses_directives_as_top_level_items() {
        let source = "$option --dallow-internal\n$start_module# C\n$pragma{enabled = true, nested = {level = 2}, flags = [boot, false]}\nval x : int";
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        assert!(matches!(
            &ast.items[0].0,
            TopLevelDef::Directive(DirectiveDef { name, payload, .. })
                if name.0 == "option"
                    && payload.as_ref().map(|payload| payload.0.as_str()) == Some(" --dallow-internal")
        ));
        assert!(matches!(
            &ast.items[1].0,
            TopLevelDef::Directive(DirectiveDef { name, payload, .. })
                if name.0 == "start_module"
                    && payload.as_ref().map(|payload| payload.0.as_str()) == Some("# C")
        ));
        assert!(matches!(
            &ast.items[2].0,
            TopLevelDef::Directive(DirectiveDef {
                name,
                payload: None,
                structured_payload: Some((AttributeData::Object(entries), _)),
                ..
            }) if name.0 == "pragma"
                && entries.len() == 3
                && matches!(entries[0].0.value.0, AttributeData::Bool(true))
                && matches!(entries[1].0.value.0, AttributeData::Object(_))
                && matches!(entries[2].0.value.0, AttributeData::Array(_))
        ));
        assert!(matches!(&ast.items[3].0, TopLevelDef::CallableSpec(_)));
    }

    #[test]
    fn parses_function_rec_measures_and_and_clauses() {
        let source = r#"
function { xs => dec(xs) } foo forall 'n. (x if guard(x)) -> bits('n) = body(x)
and $[alt] Private foo y = other(y)
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        let TopLevelDef::CallableDef(def) = &ast.items[0].0 else {
            panic!("expected callable def");
        };
        let rec_measure = def.rec_measure.as_ref().expect("rec measure");
        assert!(matches!(&rec_measure.0.body.0, Expr::Call(_)));
        assert_eq!(def.clauses.len(), 2);
        assert!(matches!(
            &def.clauses[0].0.quantifier,
            Some(CallableQuantifier { vars, .. }) if vars.len() == 1 && vars[0].name.0 == "'n"
        ));
        assert!(matches!(&def.clauses[0].0.guard, Some((Expr::Call(_), _))));
        assert!(matches!(
            &def.clauses[0].0.return_ty,
            Some((TypeExpr::App { callee, .. }, _)) if callee.0 == "bits"
        ));
        assert!(matches!(
            &def.clauses[1].0.modifiers,
            DefModifiers { is_private: true, attrs } if attrs.len() == 1 && attrs[0].0.name.0 == "alt"
        ));
        assert!(matches!(&def.clauses[1].0.body, Some((Expr::Call(_), _))));
    }

    #[test]
    fn parses_riscv_style_quantifiers_and_existentials() {
        let source = r#"
private val split_misaligned : forall 'width, 'width > 0.
  (virtaddr, int('width)) -> {'n 'bytes, 'width == 'n * 'bytes & 'bytes > 0. (int('n), int('bytes))}

private function write_pte forall 'n, 'n in {4, 8} . (
  paddr    : physaddr,
  pte_size : int('n),
) -> MemoryOpResult(bits(8 * 'n)) = true
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        let TopLevelDef::CallableSpec(spec) = &ast.items[0].0 else {
            panic!("expected callable spec");
        };
        let TypeExpr::Forall {
            vars,
            constraint: Some(width_constraint),
            body,
        } = &spec.signature.0
        else {
            panic!("expected forall signature");
        };
        assert_eq!(vars.len(), 1);
        assert_eq!(vars[0].name.0, "'width");
        assert!(matches!(
            width_constraint.0,
            TypeExpr::Infix { ref lhs, ref op, .. }
                if op.0 == ">" && matches!(lhs.0, TypeExpr::TypeVar(ref name) if name == "'width")
        ));
        let TypeExpr::Arrow { ret, .. } = &body.0 else {
            panic!("expected arrow body");
        };
        let TypeExpr::Existential {
            binder: first_binder,
            constraint: None,
            body: inner_existential,
        } = &ret.0
        else {
            panic!("expected outer existential");
        };
        assert_eq!(first_binder.name.0, "'n");
        let TypeExpr::Existential {
            binder: second_binder,
            constraint: Some(bytes_constraint),
            body: existential_body,
        } = &inner_existential.0
        else {
            panic!("expected inner existential");
        };
        assert_eq!(second_binder.name.0, "'bytes");
        assert!(matches!(bytes_constraint.0, TypeExpr::Infix { .. }));
        assert!(matches!(existential_body.0, TypeExpr::Tuple(_)));

        let TopLevelDef::CallableDef(def) = &ast.items[1].0 else {
            panic!("expected callable def");
        };
        let quantifier = def.clauses[0].0.quantifier.as_ref().expect("quantifier");
        assert_eq!(quantifier.vars.len(), 1);
        assert_eq!(quantifier.vars[0].name.0, "'n");
        let constraint = quantifier.constraint.as_ref().expect("constraint");
        assert!(matches!(
            constraint.0,
            TypeExpr::Infix { ref lhs, ref op, ref rhs }
                if op.0 == "in"
                    && matches!(lhs.0, TypeExpr::TypeVar(ref name) if name == "'n")
                    && matches!(rhs.0, TypeExpr::Set(ref items) if items.len() == 2)
        ));
    }

    #[test]
    fn parses_riscv_style_foreach_ranges_and_wildcard_mapping_arms() {
        let source = r#"
mapping privLevel_bits : int <-> int = {
  forwards _ => 0,
}

function walk(sys_pmp_count) = {
  foreach (i from 0 to sys_pmp_count - 1 by 2 - 1) {
    i
  };
  0
}
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = parse_source(&tokens).into_result().unwrap();

        let TopLevelDef::CallableDef(mapping) = &ast.items[0].0 else {
            panic!("expected mapping def");
        };
        let mapping_body = mapping.mapping_body.as_ref().expect("mapping body");
        assert!(matches!(
            mapping_body.arms[0].0.lhs.0,
            Expr::Ident(ref name) if name == "_"
        ));

        let TopLevelDef::CallableDef(def) = &ast.items[1].0 else {
            panic!("expected callable def");
        };
        let Some((Expr::Block(items), _)) = &def.body else {
            panic!("expected block body");
        };
        let BlockItem::Expr((Expr::Foreach(foreach), _)) = &items[0].0 else {
            panic!("expected foreach expr");
        };
        assert!(matches!(
            foreach.end.0,
            Expr::Infix { ref op, .. } if op.0 == "-"
        ));
        assert!(matches!(
            foreach.step,
            Some(ref step) if matches!(step.0, Expr::Infix { ref op, .. } if op.0 == "-")
        ));
    }
}
