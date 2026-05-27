//! Token type classification helpers.

pub(super) fn token_type_index(
    token: &parser::Token,
    previous: Option<&parser::Token>,
) -> Option<u32> {
    let idx = match token {
        // NOTE: Comments are skipped by the lexer (hand_lexer.rs:28-55)
        // and don't appear as tokens. Comment highlighting is handled by
        // the editor's built-in TextMate grammar. index 12 reserved for
        // future use if comments become tokenized.
        parser::Token::Id(_) => match previous {
            Some(parser::Token::KwFunction)
            | Some(parser::Token::KwVal)
            | Some(parser::Token::KwMapping)
            | Some(parser::Token::KwOverload) => 1,
            Some(parser::Token::KwType) | Some(parser::Token::KwUnion) => 2,
            Some(parser::Token::KwStruct) | Some(parser::Token::KwBitfield) => 9, // struct
            Some(parser::Token::KwEnum) => 3,
            Some(parser::Token::KwRegister) => 13, // register
            Some(parser::Token::KwLet) | Some(parser::Token::KwVar) => 4,
            _ => 4,
        },
        parser::Token::TyVal(_) => 5,
        parser::Token::String(_) => 6,
        parser::Token::Num(_)
        | parser::Token::Real(_)
        | parser::Token::Bin(_)
        | parser::Token::Hex(_) => 7,
        _ if token_is_keyword(token) => 0,
        _ if matches!(
            token,
            parser::Token::RightArrow
                | parser::Token::LeftArrow
                | parser::Token::FatRightArrow
                | parser::Token::DoubleArrow
                | parser::Token::LessThan
                | parser::Token::GreaterThan
                | parser::Token::LessThanOrEqualTo
                | parser::Token::GreaterThanOrEqualTo
                | parser::Token::Modulus
                | parser::Token::Multiply
                | parser::Token::Divide
                | parser::Token::Equal
                | parser::Token::EqualTo
                | parser::Token::NotEqualTo
                | parser::Token::And
                | parser::Token::Or
                | parser::Token::Scope
                | parser::Token::Plus
                | parser::Token::Minus
        ) =>
        {
            8
        }
        _ => return None,
    };
    Some(idx)
}

pub(super) fn token_is_keyword(token: &parser::Token) -> bool {
    matches!(
        token,
        parser::Token::KwAnd
            | parser::Token::KwAs
            | parser::Token::KwAssert
            | parser::Token::KwBackwards
            | parser::Token::KwBarr
            | parser::Token::KwBitfield
            | parser::Token::KwBitone
            | parser::Token::KwBitzero
            | parser::Token::KwBool
            | parser::Token::KwBy
            | parser::Token::KwCast
            | parser::Token::KwCatch
            | parser::Token::KwClause
            | parser::Token::KwConfiguration
            | parser::Token::KwConstant
            | parser::Token::KwConstraint
            | parser::Token::KwDec
            | parser::Token::KwDefault
            | parser::Token::KwDepend
            | parser::Token::KwDo
            | parser::Token::KwDownto
            | parser::Token::KwEamem
            | parser::Token::KwEffect
            | parser::Token::KwElse
            | parser::Token::KwEnd
            | parser::Token::KwEnum
            | parser::Token::KwEscape
            | parser::Token::KwExit
            | parser::Token::KwExmem
            | parser::Token::KwFalse
            | parser::Token::KwForall
            | parser::Token::KwForeach
            | parser::Token::KwForwards
            | parser::Token::KwFrom
            | parser::Token::KwFunction
            | parser::Token::KwIf
            | parser::Token::KwImpl
            | parser::Token::KwIn
            | parser::Token::KwInc
            | parser::Token::KwInfix
            | parser::Token::KwInfixl
            | parser::Token::KwInfixr
            | parser::Token::KwInstantiation
            | parser::Token::KwInt
            | parser::Token::KwLet
            | parser::Token::KwMapping
            | parser::Token::KwMatch
            | parser::Token::KwMonadic
            | parser::Token::KwMutual
            | parser::Token::KwMwv
            | parser::Token::KwNewtype
            | parser::Token::KwNondet
            | parser::Token::KwOrder
            | parser::Token::KwOutcome
            | parser::Token::KwOverload
            | parser::Token::KwPrivate
            | parser::Token::KwPure
            | parser::Token::KwRef
            | parser::Token::KwRegister
            | parser::Token::KwRepeat
            | parser::Token::KwReturn
            | parser::Token::KwRmem
            | parser::Token::KwRreg
            | parser::Token::KwScattered
            | parser::Token::KwSizeof
            | parser::Token::KwStruct
            | parser::Token::KwTerminationMeasure
            | parser::Token::KwThen
            | parser::Token::KwThrow
            | parser::Token::KwTo
            | parser::Token::KwTrue
            | parser::Token::KwTry
            | parser::Token::KwType
            | parser::Token::KwTypeUpper
            | parser::Token::KwUndef
            | parser::Token::KwUndefined
            | parser::Token::KwUnion
            | parser::Token::KwUnspec
            | parser::Token::KwUntil
            | parser::Token::KwVal
            | parser::Token::KwVar
            | parser::Token::KwWhen
            | parser::Token::KwWhile
            | parser::Token::KwWith
            | parser::Token::KwWmem
            | parser::Token::KwWreg
    )
}
