use crate::state::File;
use tower_lsp::lsp_types::{
    Range, SemanticToken, SemanticTokenType, SemanticTokens, SemanticTokensDelta,
    SemanticTokensEdit, SemanticTokensFullOptions, SemanticTokensLegend, SemanticTokensOptions,
    WorkDoneProgressOptions,
};

const TOKEN_TYPES: &[SemanticTokenType] = &[
    SemanticTokenType::KEYWORD,        // 0
    SemanticTokenType::FUNCTION,       // 1
    SemanticTokenType::TYPE,           // 2
    SemanticTokenType::ENUM,           // 3
    SemanticTokenType::VARIABLE,       // 4
    SemanticTokenType::TYPE_PARAMETER, // 5
    SemanticTokenType::STRING,         // 6
    SemanticTokenType::NUMBER,         // 7
    SemanticTokenType::OPERATOR,       // 8
];

pub(crate) fn semantic_tokens_options() -> SemanticTokensOptions {
    SemanticTokensOptions {
        work_done_progress_options: WorkDoneProgressOptions {
            work_done_progress: Some(false),
        },
        legend: SemanticTokensLegend {
            token_types: TOKEN_TYPES.to_vec(),
            token_modifiers: Vec::new(),
        },
        range: Some(true),
        full: Some(SemanticTokensFullOptions::Delta { delta: Some(true) }),
    }
}

fn token_type_index(
    token: &sail_parser::Token,
    previous: Option<&sail_parser::Token>,
) -> Option<u32> {
    let idx = match token {
        sail_parser::Token::Id(_) => match previous {
            Some(sail_parser::Token::KwFunction)
            | Some(sail_parser::Token::KwVal)
            | Some(sail_parser::Token::KwMapping)
            | Some(sail_parser::Token::KwOverload) => 1,
            Some(sail_parser::Token::KwType)
            | Some(sail_parser::Token::KwStruct)
            | Some(sail_parser::Token::KwUnion)
            | Some(sail_parser::Token::KwBitfield) => 2,
            Some(sail_parser::Token::KwEnum) => 3,
            Some(sail_parser::Token::KwRegister)
            | Some(sail_parser::Token::KwLet)
            | Some(sail_parser::Token::KwVar) => 4,
            _ => 4,
        },
        sail_parser::Token::TyVal(_) => 5,
        sail_parser::Token::String(_) => 6,
        sail_parser::Token::Num(_)
        | sail_parser::Token::Real(_)
        | sail_parser::Token::Bin(_)
        | sail_parser::Token::Hex(_) => 7,
        _ if token_is_keyword(token) => 0,
        _ if matches!(
            token,
            sail_parser::Token::RightArrow
                | sail_parser::Token::LeftArrow
                | sail_parser::Token::FatRightArrow
                | sail_parser::Token::DoubleArrow
                | sail_parser::Token::LessThan
                | sail_parser::Token::GreaterThan
                | sail_parser::Token::LessThanOrEqualTo
                | sail_parser::Token::GreaterThanOrEqualTo
                | sail_parser::Token::Modulus
                | sail_parser::Token::Multiply
                | sail_parser::Token::Divide
                | sail_parser::Token::Equal
                | sail_parser::Token::EqualTo
                | sail_parser::Token::NotEqualTo
                | sail_parser::Token::And
                | sail_parser::Token::Or
                | sail_parser::Token::Scope
                | sail_parser::Token::Plus
                | sail_parser::Token::Minus
        ) =>
        {
            8
        }
        _ => return None,
    };
    Some(idx)
}

fn token_is_keyword(token: &sail_parser::Token) -> bool {
    matches!(
        token,
        sail_parser::Token::KwAnd
            | sail_parser::Token::KwAs
            | sail_parser::Token::KwAssert
            | sail_parser::Token::KwBackwards
            | sail_parser::Token::KwBarr
            | sail_parser::Token::KwBitfield
            | sail_parser::Token::KwBitone
            | sail_parser::Token::KwBitzero
            | sail_parser::Token::KwBool
            | sail_parser::Token::KwBy
            | sail_parser::Token::KwCast
            | sail_parser::Token::KwCatch
            | sail_parser::Token::KwClause
            | sail_parser::Token::KwConfiguration
            | sail_parser::Token::KwConstant
            | sail_parser::Token::KwConstraint
            | sail_parser::Token::KwDec
            | sail_parser::Token::KwDefault
            | sail_parser::Token::KwDepend
            | sail_parser::Token::KwDo
            | sail_parser::Token::KwEamem
            | sail_parser::Token::KwEffect
            | sail_parser::Token::KwElse
            | sail_parser::Token::KwEnd
            | sail_parser::Token::KwEnum
            | sail_parser::Token::KwEscape
            | sail_parser::Token::KwExit
            | sail_parser::Token::KwExmem
            | sail_parser::Token::KwFalse
            | sail_parser::Token::KwForall
            | sail_parser::Token::KwForeach
            | sail_parser::Token::KwForwards
            | sail_parser::Token::KwFunction
            | sail_parser::Token::KwIf
            | sail_parser::Token::KwImpl
            | sail_parser::Token::KwIn
            | sail_parser::Token::KwInc
            | sail_parser::Token::KwInfix
            | sail_parser::Token::KwInfixl
            | sail_parser::Token::KwInfixr
            | sail_parser::Token::KwInstantiation
            | sail_parser::Token::KwInt
            | sail_parser::Token::KwLet
            | sail_parser::Token::KwMapping
            | sail_parser::Token::KwMatch
            | sail_parser::Token::KwMonadic
            | sail_parser::Token::KwMutual
            | sail_parser::Token::KwMwv
            | sail_parser::Token::KwNewtype
            | sail_parser::Token::KwNondet
            | sail_parser::Token::KwOrder
            | sail_parser::Token::KwOutcome
            | sail_parser::Token::KwOverload
            | sail_parser::Token::KwPure
            | sail_parser::Token::KwRef
            | sail_parser::Token::KwRegister
            | sail_parser::Token::KwRepeat
            | sail_parser::Token::KwReturn
            | sail_parser::Token::KwRmem
            | sail_parser::Token::KwRreg
            | sail_parser::Token::KwScattered
            | sail_parser::Token::KwSizeof
            | sail_parser::Token::KwStruct
            | sail_parser::Token::KwTerminationMeasure
            | sail_parser::Token::KwThen
            | sail_parser::Token::KwThrow
            | sail_parser::Token::KwTrue
            | sail_parser::Token::KwTry
            | sail_parser::Token::KwType
            | sail_parser::Token::KwTypeUpper
            | sail_parser::Token::KwUndef
            | sail_parser::Token::KwUndefined
            | sail_parser::Token::KwUnion
            | sail_parser::Token::KwUnspec
            | sail_parser::Token::KwUntil
            | sail_parser::Token::KwVal
            | sail_parser::Token::KwVar
            | sail_parser::Token::KwWhile
            | sail_parser::Token::KwWith
            | sail_parser::Token::KwWmem
            | sail_parser::Token::KwWreg
    )
}

fn semantic_tokens_result_id(file: &File, token_count: usize) -> String {
    format!("{:x}:{:x}", file.source.text().len(), token_count)
}

fn compute_semantic_tokens_filtered(file: &File, range: Option<&Range>) -> SemanticTokens {
    let mut result = Vec::<SemanticToken>::new();
    let mut prev_line = 0_u32;
    let mut prev_start = 0_u32;
    let mut first = true;

    let Some(tokens) = file.tokens.as_ref() else {
        return SemanticTokens {
            result_id: None,
            data: result,
        };
    };

    let mut prev_token: Option<&sail_parser::Token> = None;
    for (token, span) in tokens {
        let Some(tt) = token_type_index(token, prev_token) else {
            prev_token = Some(token);
            continue;
        };
        let start = file.source.position_at(span.start);
        let end = file.source.position_at(span.end);
        if start.line != end.line || end.character <= start.character {
            prev_token = Some(token);
            continue;
        }
        if let Some(r) = range {
            if start.line < r.start.line || start.line > r.end.line {
                prev_token = Some(token);
                continue;
            }
        }

        let delta_line = if first {
            start.line
        } else {
            start.line - prev_line
        };
        let delta_start = if first {
            start.character
        } else if delta_line == 0 {
            start.character - prev_start
        } else {
            start.character
        };

        result.push(SemanticToken {
            delta_line,
            delta_start,
            length: end.character - start.character,
            token_type: tt,
            token_modifiers_bitset: 0,
        });
        first = false;
        prev_line = start.line;
        prev_start = start.character;
        prev_token = Some(token);
    }

    SemanticTokens {
        result_id: Some(semantic_tokens_result_id(file, result.len())),
        data: result,
    }
}

pub(crate) fn compute_semantic_tokens(file: &File) -> SemanticTokens {
    compute_semantic_tokens_filtered(file, None)
}

pub(crate) fn compute_semantic_tokens_range(file: &File, range: &Range) -> SemanticTokens {
    compute_semantic_tokens_filtered(file, Some(range))
}

pub(crate) fn compute_semantic_tokens_delta(
    previous: &SemanticTokens,
    current: &SemanticTokens,
) -> SemanticTokensDelta {
    let previous_data = &previous.data;
    let current_data = &current.data;

    let mut prefix = 0_usize;
    let prefix_limit = previous_data.len().min(current_data.len());
    while prefix < prefix_limit && previous_data[prefix] == current_data[prefix] {
        prefix += 1;
    }

    let mut suffix = 0_usize;
    while suffix + prefix < previous_data.len()
        && suffix + prefix < current_data.len()
        && previous_data[previous_data.len() - 1 - suffix]
            == current_data[current_data.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let edits = if prefix == previous_data.len() && prefix == current_data.len() {
        Vec::new()
    } else {
        vec![SemanticTokensEdit {
            start: prefix as u32,
            delete_count: (previous_data.len() - prefix - suffix) as u32,
            data: Some(current_data[prefix..(current_data.len() - suffix)].to_vec()),
        }]
    };

    SemanticTokensDelta {
        result_id: current.result_id.clone(),
        edits,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::File;

    #[test]
    fn emits_non_empty_tokens() {
        let file = File::new("function f(x) = x + 1".to_string());
        let tokens = compute_semantic_tokens(&file);
        assert!(!tokens.data.is_empty());
    }

    #[test]
    fn includes_keyword_and_number_tokens() {
        let file = File::new("let x = 42".to_string());
        let tokens = compute_semantic_tokens(&file);
        assert!(tokens.data.iter().any(|t| t.token_type == 0));
        assert!(tokens.data.iter().any(|t| t.token_type == 7));
    }

    #[test]
    fn range_tokens_are_subset_of_full() {
        let file = File::new("let x = 1\nlet y = 2\n".to_string());
        let full = compute_semantic_tokens(&file);
        let range = compute_semantic_tokens_range(
            &file,
            &Range::new(
                tower_lsp::lsp_types::Position::new(0, 0),
                tower_lsp::lsp_types::Position::new(0, 10),
            ),
        );
        assert!(range.data.len() < full.data.len());
    }

    #[test]
    fn classifies_keywords_without_debug_string_hack() {
        assert_eq!(
            token_type_index(&sail_parser::Token::KwFunction, None),
            Some(0)
        );
    }

    #[test]
    fn semantic_token_delta_uses_incremental_edit() {
        let old_file = File::new("let x = 1".to_string());
        let new_file = File::new("let xyz = 100".to_string());
        let previous = compute_semantic_tokens(&old_file);
        let current = compute_semantic_tokens(&new_file);
        let delta = compute_semantic_tokens_delta(&previous, &current);
        assert!(!delta.edits.is_empty());
    }
}
