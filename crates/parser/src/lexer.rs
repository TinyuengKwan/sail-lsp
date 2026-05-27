//! Sail token types and span definition.
//!
//! chumsky dependency removed. Span is now a custom type.
//! The hand-written lexer lives in `hand_lexer.rs`.
use std::fmt;

/// Byte-offset span in source text. Replaces `chumsky::SimpleSpan<usize>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Span {
    /// Start byte offset (inclusive).
    pub start: usize,
    /// End byte offset (exclusive).
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

// TODO: Make tokens zero copy &str when we have a parser as well as a lexer.
// For now they are String to keep things simple.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Token {
    // Identifiers
    Id(String),
    TyVal(String), // 'identifier (the ' is discarded)

    // Number literals.
    Bin(String),  // 0b010101 (the 0b is discarded)
    Hex(String),  // 0xDEAD32 (the 0x is discarded)
    Num(String),  // -123
    Real(String), //-034.432

    // String literal.
    String(String),

    // Multiline string literal (triple-quoted).
    MultilineString(String),

    // Operators and control characters.
    Dollar,
    Directive {
        name: String,
        payload: Option<String>,
    },
    StructuredDirectiveStart(String),
    Hash,
    LeftBracket,        // (
    RightBracket,       // )
    LeftSquareBracket,  // [
    RightSquareBracket, // ]
    LeftCurlyBracket,   // {
    RightCurlyBracket,  // }
    RightArrow,         // ->
    LeftArrow,          // <-
    FatRightArrow,      // =>
    DoubleArrow,        // <->
    ColonEqual,         // :=
    Comma,
    Colon,
    Semicolon,
    Dot,
    Caret, // ^
    At,    // @
    LessThan,
    GreaterThan,
    LessThanOrEqualTo,
    GreaterThanOrEqualTo,
    Modulus,    // %
    Multiply,   // *
    Divide,     // /
    Equal,      // =
    EqualTo,    // ==
    NotEqualTo, // !=
    And,        // &
    Or,         // |
    Scope,      // ::
    Plus,
    Minus,
    LeftCurlyBar,   // {|
    RightCurlyBar,  // |}
    LeftSquareBar,  // [|
    RightSquareBar, // |]
    Underscore,     // _
    Unit,           // ()

    /// Infix operator with subscript suffix (e.g. `<_s`, `<=_u`, `>_si`).
    /// Upstream lexer.mll:186-189: `operatorn` rule allows `_ident` suffix.
    InfixOp(String),

    // Keywords.
    KwAnd,
    KwAs,
    KwAssert,
    KwBackwards,
    KwBarr,
    KwBitfield,
    KwBitone,
    KwBitzero,
    KwBool,
    KwBy,
    KwCast,
    KwCatch,
    KwCase,
    KwClause,
    KwConfig,
    KwConfiguration,
    KwConstant,
    KwConstraint,
    KwDec,
    KwDefault,
    KwDepend,
    KwDo,
    KwDownto,
    KwEamem,
    KwEffect,
    KwElse,
    KwEnd,
    KwEnum,
    KwEscape,
    KwExit,
    KwExmem,
    KwFalse,
    KwForall,
    KwForeach,
    KwForwards,
    KwFrom,
    KwFunction,
    KwIf,
    KwImpl,
    KwIn,
    KwInc,
    KwInfix,
    KwInfixl,
    KwInfixr,
    KwInstantiation,
    KwInt,
    KwLet,
    KwMapping,
    KwMatch,
    KwMonadic,
    KwMutual,
    KwMwv,
    KwNewtype,
    KwNondet,
    KwOrder,
    KwOutcome,
    KwOverload,
    KwPrivate,
    KwPure,
    KwRef,
    KwRegister,
    KwRepeat,
    KwReturn,
    KwRmem,
    KwRreg,
    KwScattered,
    KwSizeof,
    KwStruct,
    KwSwitch,
    KwTerminationMeasure,
    KwThen,
    KwThrow,
    KwTo,
    KwTrue,
    KwTry,
    KwType,      // type
    KwTypeUpper, // Type
    KwUndef,
    KwUndefined,
    KwUnion,
    KwUnspec,
    KwUntil,
    KwVal,
    KwVar,
    KwWhen,
    KwWhile,
    KwWith,
    KwWmem,
    KwWreg,
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            // Identifiers.
            Token::Id(s) => write!(f, "{}", s),
            Token::TyVal(s) => write!(f, "{}", s),

            // Numbers literals.
            Token::Bin(s) => write!(f, "{}", s),
            Token::Hex(s) => write!(f, "{}", s),
            Token::Num(s) => write!(f, "{}", s),
            Token::Real(s) => write!(f, "{}", s),

            // String literal.
            Token::String(s) => write!(f, "{}", s),
            Token::MultilineString(s) => write!(f, "\"\"\"{}\"\"\"", s),

            // Operators and other control characters.
            Token::Dollar => write!(f, "$"),
            Token::Directive { name, payload } => {
                write!(f, "${name}")?;
                if let Some(payload) = payload {
                    write!(f, "{payload}")?;
                }
                Ok(())
            }
            Token::StructuredDirectiveStart(name) => write!(f, "${name}{{"),
            Token::Hash => write!(f, "#"),
            Token::LeftBracket => write!(f, "("),
            Token::RightBracket => write!(f, ")"),
            Token::LeftSquareBracket => write!(f, "["),
            Token::RightSquareBracket => write!(f, "]"),
            Token::LeftCurlyBracket => write!(f, "{{"),
            Token::RightCurlyBracket => write!(f, "}}"),
            Token::RightArrow => write!(f, "->"),
            Token::LeftArrow => write!(f, "<-"),
            Token::FatRightArrow => write!(f, "=>"),
            Token::DoubleArrow => write!(f, "<->"),
            Token::ColonEqual => write!(f, ":="),
            Token::Comma => write!(f, ","),
            Token::Colon => write!(f, ":"),
            Token::Semicolon => write!(f, ";"),
            Token::Dot => write!(f, "."),
            Token::Caret => write!(f, "^"),
            Token::At => write!(f, "@"),
            Token::LessThan => write!(f, "<"),
            Token::GreaterThan => write!(f, ">"),
            Token::LessThanOrEqualTo => write!(f, "<="),
            Token::GreaterThanOrEqualTo => write!(f, ">="),
            Token::Modulus => write!(f, "%"),
            Token::Multiply => write!(f, "*"),
            Token::Divide => write!(f, "/"),
            Token::Equal => write!(f, "="),
            Token::EqualTo => write!(f, "=="),
            Token::NotEqualTo => write!(f, "!="),
            Token::And => write!(f, "&"),
            Token::Or => write!(f, "|"),
            Token::Scope => write!(f, "::"),
            Token::Plus => write!(f, "+"),
            Token::Minus => write!(f, "-"),
            Token::LeftCurlyBar => write!(f, "{{|"),
            Token::RightCurlyBar => write!(f, "|}}"),
            Token::LeftSquareBar => write!(f, "[|"),
            Token::RightSquareBar => write!(f, "|]"),
            Token::Underscore => write!(f, "_"),
            Token::Unit => write!(f, "()"),
            Token::InfixOp(op) => write!(f, "{}", op),

            // Keywords.
            Token::KwAnd => write!(f, "and"),
            Token::KwAs => write!(f, "as"),
            Token::KwAssert => write!(f, "assert"),
            Token::KwBackwards => write!(f, "backwards"),
            Token::KwBarr => write!(f, "barr"),
            Token::KwBitfield => write!(f, "bitfield"),
            Token::KwBitone => write!(f, "bitone"),
            Token::KwBitzero => write!(f, "bitzero"),
            Token::KwBool => write!(f, "Bool"),
            Token::KwBy => write!(f, "by"),
            Token::KwCast => write!(f, "cast"),
            Token::KwCatch => write!(f, "catch"),
            Token::KwCase => write!(f, "case"),
            Token::KwClause => write!(f, "clause"),
            Token::KwConfig => write!(f, "config"),
            Token::KwConfiguration => write!(f, "configuration"),
            Token::KwConstant => write!(f, "constant"),
            Token::KwConstraint => write!(f, "constraint"),
            Token::KwDec => write!(f, "dec"),
            Token::KwDefault => write!(f, "default"),
            Token::KwDepend => write!(f, "depend"),
            Token::KwDo => write!(f, "do"),
            Token::KwDownto => write!(f, "downto"),
            Token::KwEamem => write!(f, "eamem"),
            Token::KwEffect => write!(f, "effect"),
            Token::KwElse => write!(f, "else"),
            Token::KwEnd => write!(f, "end"),
            Token::KwEnum => write!(f, "enum"),
            Token::KwEscape => write!(f, "escape"),
            Token::KwExit => write!(f, "exit"),
            Token::KwExmem => write!(f, "exmem"),
            Token::KwFalse => write!(f, "false"),
            Token::KwForall => write!(f, "forall"),
            Token::KwForeach => write!(f, "foreach"),
            Token::KwForwards => write!(f, "forwards"),
            Token::KwFrom => write!(f, "from"),
            Token::KwFunction => write!(f, "function"),
            Token::KwIf => write!(f, "if"),
            Token::KwImpl => write!(f, "impl"),
            Token::KwIn => write!(f, "in"),
            Token::KwInc => write!(f, "inc"),
            Token::KwInfix => write!(f, "infix"),
            Token::KwInfixl => write!(f, "infixl"),
            Token::KwInfixr => write!(f, "infixr"),
            Token::KwInstantiation => write!(f, "instantiation"),
            Token::KwInt => write!(f, "Int"),
            Token::KwLet => write!(f, "let"),
            Token::KwMapping => write!(f, "mapping"),
            Token::KwMatch => write!(f, "match"),
            Token::KwMonadic => write!(f, "monadic"),
            Token::KwMutual => write!(f, "mutual"),
            Token::KwMwv => write!(f, "mwv"),
            Token::KwNewtype => write!(f, "newtype"),
            Token::KwNondet => write!(f, "nondet"),
            Token::KwOrder => write!(f, "Order"),
            Token::KwOutcome => write!(f, "outcome"),
            Token::KwOverload => write!(f, "overload"),
            Token::KwPrivate => write!(f, "private"),
            Token::KwPure => write!(f, "pure"),
            Token::KwRef => write!(f, "ref"),
            Token::KwRegister => write!(f, "register"),
            Token::KwRepeat => write!(f, "repeat"),
            Token::KwReturn => write!(f, "return"),
            Token::KwRmem => write!(f, "rmem"),
            Token::KwRreg => write!(f, "rreg"),
            Token::KwScattered => write!(f, "scattered"),
            Token::KwSizeof => write!(f, "sizeof"),
            Token::KwStruct => write!(f, "struct"),
            Token::KwSwitch => write!(f, "switch"),
            Token::KwTerminationMeasure => write!(f, "termination_measure"),
            Token::KwThen => write!(f, "then"),
            Token::KwThrow => write!(f, "throw"),
            Token::KwTo => write!(f, "to"),
            Token::KwTrue => write!(f, "true"),
            Token::KwTry => write!(f, "try"),
            Token::KwType => write!(f, "type"),
            Token::KwTypeUpper => write!(f, "Type"),
            Token::KwUndef => write!(f, "undef"),
            Token::KwUndefined => write!(f, "undefined"),
            Token::KwUnion => write!(f, "union"),
            Token::KwUnspec => write!(f, "unspec"),
            Token::KwUntil => write!(f, "until"),
            Token::KwVal => write!(f, "val"),
            Token::KwVar => write!(f, "var"),
            Token::KwWhen => write!(f, "when"),
            Token::KwWhile => write!(f, "while"),
            Token::KwWith => write!(f, "with"),
            Token::KwWmem => write!(f, "wmem"),
            Token::KwWreg => write!(f, "wreg"),
        }
    }
}

// chumsky lexer functions (ident, n_digits, lexer) and tests DELETED.
// The hand-written lexer in hand_lexer.rs replaces all chumsky tokenization.
// Only the Token enum, Span struct, and Display impl remain in this file.
