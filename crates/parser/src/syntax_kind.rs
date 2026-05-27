//! `SyntaxKind` — wrapper module over generated enum.
//!
//! The `SyntaxKind` enum itself lives in `generated.rs` (produced
//! from `sail.ungram`). This file adds hand-written methods and
//! the rowan `Language` integration.

#[rustfmt::skip]
mod generated;

use crate::lexer::Token;

#[allow(unreachable_pub)]
pub use self::generated::SyntaxKind;

impl From<u16> for SyntaxKind {
    #[inline]
    fn from(d: u16) -> SyntaxKind {
        assert!(d <= (SyntaxKind::__LAST as u16));
        // SAFETY: SyntaxKind is repr(u16) and d is in range.
        unsafe { std::mem::transmute::<u16, SyntaxKind>(d) }
    }
}

impl From<SyntaxKind> for u16 {
    #[inline]
    fn from(k: SyntaxKind) -> u16 {
        k as u16
    }
}

impl SyntaxKind {
    /// Convert a lexer `Token` to the corresponding `SyntaxKind`.
    pub fn from_token(token: &Token) -> Self {
        match token {
            Token::Id(_) => SyntaxKind::IDENT,
            Token::TyVal(_) => SyntaxKind::TY_VAR,
            Token::Bin(_) => SyntaxKind::BIN_LIT,
            Token::Hex(_) => SyntaxKind::HEX_LIT,
            Token::Num(_) => SyntaxKind::NUM_LIT,
            Token::Real(_) => SyntaxKind::REAL_LIT,
            Token::String(_) => SyntaxKind::STRING_LIT,
            Token::MultilineString(_) => SyntaxKind::MULTILINE_STRING_LIT,
            Token::Dollar => SyntaxKind::DOLLAR,
            Token::Directive { .. } => SyntaxKind::DIRECTIVE,
            Token::StructuredDirectiveStart(_) => SyntaxKind::STRUCTURED_DIRECTIVE_START,
            Token::Hash => SyntaxKind::HASH,
            Token::LeftBracket => SyntaxKind::L_PAREN,
            Token::RightBracket => SyntaxKind::R_PAREN,
            Token::LeftSquareBracket => SyntaxKind::L_BRACK,
            Token::RightSquareBracket => SyntaxKind::R_BRACK,
            Token::LeftCurlyBracket => SyntaxKind::L_CURLY,
            Token::RightCurlyBracket => SyntaxKind::R_CURLY,
            Token::RightArrow => SyntaxKind::R_ARROW,
            Token::LeftArrow => SyntaxKind::L_ARROW,
            Token::FatRightArrow => SyntaxKind::FAT_R_ARROW,
            Token::DoubleArrow => SyntaxKind::DOUBLE_ARROW,
            Token::ColonEqual => SyntaxKind::COLON_EQ,
            Token::Comma => SyntaxKind::COMMA,
            Token::Colon => SyntaxKind::COLON,
            Token::Semicolon => SyntaxKind::SEMICOLON,
            Token::Dot => SyntaxKind::DOT,
            Token::Caret => SyntaxKind::CARET,
            Token::At => SyntaxKind::AT,
            Token::LessThan => SyntaxKind::L_ANGLE,
            Token::GreaterThan => SyntaxKind::R_ANGLE,
            Token::LessThanOrEqualTo => SyntaxKind::LE,
            Token::GreaterThanOrEqualTo => SyntaxKind::GE,
            Token::Modulus => SyntaxKind::PERCENT,
            Token::Multiply => SyntaxKind::STAR,
            Token::Divide => SyntaxKind::SLASH,
            Token::Equal => SyntaxKind::EQ,
            Token::EqualTo => SyntaxKind::EQ_EQ,
            Token::NotEqualTo => SyntaxKind::NEQ,
            Token::And => SyntaxKind::AMP,
            Token::Or => SyntaxKind::PIPE,
            Token::Scope => SyntaxKind::SCOPE,
            Token::Plus => SyntaxKind::PLUS,
            Token::Minus => SyntaxKind::MINUS,
            Token::LeftCurlyBar => SyntaxKind::L_CURLY_BAR,
            Token::RightCurlyBar => SyntaxKind::R_CURLY_BAR,
            Token::LeftSquareBar => SyntaxKind::L_BRACKET_BAR,
            Token::RightSquareBar => SyntaxKind::R_BRACKET_BAR,
            Token::Underscore => SyntaxKind::UNDERSCORE,
            Token::Unit => SyntaxKind::UNIT,
            Token::InfixOp(_) => SyntaxKind::OP_IDENT,
            // Keywords
            Token::KwAnd => SyntaxKind::KW_AND,
            Token::KwAs => SyntaxKind::KW_AS,
            Token::KwAssert => SyntaxKind::KW_ASSERT,
            Token::KwBackwards => SyntaxKind::KW_BACKWARDS,
            Token::KwBarr => SyntaxKind::KW_BARR,
            Token::KwBitfield => SyntaxKind::KW_BITFIELD,
            Token::KwBitone => SyntaxKind::KW_BITONE,
            Token::KwBitzero => SyntaxKind::KW_BITZERO,
            Token::KwBool => SyntaxKind::KW_BOOL,
            Token::KwBy => SyntaxKind::KW_BY,
            Token::KwCast => SyntaxKind::KW_CAST,
            Token::KwCatch => SyntaxKind::KW_CATCH,
            Token::KwCase => SyntaxKind::KW_CASE,
            Token::KwClause => SyntaxKind::KW_CLAUSE,
            Token::KwConfig => SyntaxKind::KW_CONFIG,
            Token::KwConfiguration => SyntaxKind::KW_CONFIGURATION,
            Token::KwConstant => SyntaxKind::KW_CONSTANT,
            Token::KwConstraint => SyntaxKind::KW_CONSTRAINT,
            Token::KwDec => SyntaxKind::KW_DEC,
            Token::KwDefault => SyntaxKind::KW_DEFAULT,
            Token::KwDepend => SyntaxKind::KW_DEPEND,
            Token::KwDo => SyntaxKind::KW_DO,
            Token::KwDownto => SyntaxKind::KW_DOWNTO,
            Token::KwEamem => SyntaxKind::KW_EAMEM,
            Token::KwEffect => SyntaxKind::KW_EFFECT,
            Token::KwElse => SyntaxKind::KW_ELSE,
            Token::KwEnd => SyntaxKind::KW_END,
            Token::KwEnum => SyntaxKind::KW_ENUM,
            Token::KwEscape => SyntaxKind::KW_ESCAPE,
            Token::KwExit => SyntaxKind::KW_EXIT,
            Token::KwExmem => SyntaxKind::KW_EXMEM,
            Token::KwFalse => SyntaxKind::KW_FALSE,
            Token::KwForall => SyntaxKind::KW_FORALL,
            Token::KwForeach => SyntaxKind::KW_FOREACH,
            Token::KwForwards => SyntaxKind::KW_FORWARDS,
            Token::KwFrom => SyntaxKind::KW_FROM,
            Token::KwFunction => SyntaxKind::KW_FUNCTION,
            Token::KwIf => SyntaxKind::KW_IF,
            Token::KwImpl => SyntaxKind::KW_IMPL,
            Token::KwIn => SyntaxKind::KW_IN,
            Token::KwInc => SyntaxKind::KW_INC,
            Token::KwInfix => SyntaxKind::KW_INFIX,
            Token::KwInfixl => SyntaxKind::KW_INFIXL,
            Token::KwInfixr => SyntaxKind::KW_INFIXR,
            Token::KwInstantiation => SyntaxKind::KW_INSTANTIATION,
            Token::KwInt => SyntaxKind::KW_INT,
            Token::KwLet => SyntaxKind::KW_LET,
            Token::KwMapping => SyntaxKind::KW_MAPPING,
            Token::KwMatch => SyntaxKind::KW_MATCH,
            Token::KwMonadic => SyntaxKind::KW_MONADIC,
            Token::KwMutual => SyntaxKind::KW_MUTUAL,
            Token::KwMwv => SyntaxKind::KW_MWV,
            Token::KwNewtype => SyntaxKind::KW_NEWTYPE,
            Token::KwNondet => SyntaxKind::KW_NONDET,
            Token::KwOrder => SyntaxKind::KW_ORDER,
            Token::KwOutcome => SyntaxKind::KW_OUTCOME,
            Token::KwOverload => SyntaxKind::KW_OVERLOAD,
            Token::KwPrivate => SyntaxKind::KW_PRIVATE,
            Token::KwPure => SyntaxKind::KW_PURE,
            Token::KwRef => SyntaxKind::KW_REF,
            Token::KwRegister => SyntaxKind::KW_REGISTER,
            Token::KwRepeat => SyntaxKind::KW_REPEAT,
            Token::KwReturn => SyntaxKind::KW_RETURN,
            Token::KwRmem => SyntaxKind::KW_RMEM,
            Token::KwRreg => SyntaxKind::KW_RREG,
            Token::KwScattered => SyntaxKind::KW_SCATTERED,
            Token::KwSizeof => SyntaxKind::KW_SIZEOF,
            Token::KwStruct => SyntaxKind::KW_STRUCT,
            Token::KwSwitch => SyntaxKind::KW_SWITCH,
            Token::KwTerminationMeasure => SyntaxKind::KW_TERMINATION_MEASURE,
            Token::KwThen => SyntaxKind::KW_THEN,
            Token::KwThrow => SyntaxKind::KW_THROW,
            Token::KwTo => SyntaxKind::KW_TO,
            Token::KwTrue => SyntaxKind::KW_TRUE,
            Token::KwTry => SyntaxKind::KW_TRY,
            Token::KwType => SyntaxKind::KW_TYPE,
            Token::KwTypeUpper => SyntaxKind::KW_TYPE_UPPER,
            Token::KwUndef => SyntaxKind::KW_UNDEF,
            Token::KwUndefined => SyntaxKind::KW_UNDEFINED,
            Token::KwUnion => SyntaxKind::KW_UNION,
            Token::KwUnspec => SyntaxKind::KW_UNSPEC,
            Token::KwUntil => SyntaxKind::KW_UNTIL,
            Token::KwVal => SyntaxKind::KW_VAL,
            Token::KwVar => SyntaxKind::KW_VAR,
            Token::KwWhen => SyntaxKind::KW_WHEN,
            Token::KwWhile => SyntaxKind::KW_WHILE,
            Token::KwWith => SyntaxKind::KW_WITH,
            Token::KwWmem => SyntaxKind::KW_WMEM,
            Token::KwWreg => SyntaxKind::KW_WREG,
        }
    }

    /// True for whitespace, comments — nodes that carry no semantic meaning.
    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            SyntaxKind::WHITESPACE | SyntaxKind::LINE_COMMENT | SyntaxKind::BLOCK_COMMENT
        )
    }

    pub fn is_type_node(self) -> bool {
        matches!(
            self,
            SyntaxKind::TYPE_NAMED
                | SyntaxKind::TYPE_VAR
                | SyntaxKind::TYPE_APP
                | SyntaxKind::TYPE_TUPLE
                | SyntaxKind::TYPE_ARROW
                | SyntaxKind::TYPE_FORALL
                | SyntaxKind::TYPE_EXISTENTIAL
                | SyntaxKind::TYPE_EFFECT
        )
    }
}

impl From<SyntaxKind> for rowan::SyntaxKind {
    fn from(kind: SyntaxKind) -> Self {
        Self(kind as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syntax_kind_fits_in_u16() {
        assert!((SyntaxKind::__LAST as u16) < u16::MAX);
    }

    #[test]
    fn every_token_maps_to_a_kind() {
        // Spot-check a few representative tokens
        assert_eq!(SyntaxKind::from_token(&Token::Id("x".into())), SyntaxKind::IDENT);
        assert_eq!(SyntaxKind::from_token(&Token::KwFunction), SyntaxKind::KW_FUNCTION);
        assert_eq!(SyntaxKind::from_token(&Token::LeftBracket), SyntaxKind::L_PAREN);
        assert_eq!(SyntaxKind::from_token(&Token::RightArrow), SyntaxKind::R_ARROW);
        assert_eq!(SyntaxKind::from_token(&Token::Num("42".into())), SyntaxKind::NUM_LIT);
    }

    #[test]
    fn trivia_classification() {
        assert!(SyntaxKind::WHITESPACE.is_trivia());
        assert!(SyntaxKind::LINE_COMMENT.is_trivia());
        assert!(SyntaxKind::BLOCK_COMMENT.is_trivia());
        assert!(!SyntaxKind::IDENT.is_trivia());
        assert!(!SyntaxKind::KW_FUNCTION.is_trivia());
    }
}
