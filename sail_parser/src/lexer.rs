//! Sail parser using Chumsky.
use chumsky::{
    combinator::Repeated,
    error::Error,
    extra::ParserExtra,
    input::{StrInput, ValueInput},
    prelude::*,
    text::Char,
    util::MaybeRef,
    Parser,
};
use std::fmt;

pub type Span = SimpleSpan<usize>;

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

/// Same as C identifiers but ? is allowed and ' is allowed after the first character.
/// Also '~' is allowed as a special identifier.
#[must_use]
pub fn ident<'a, I: ValueInput<'a> + StrInput<'a, char>, E: ParserExtra<'a, I>>(
) -> impl Parser<'a, I, &'a str, E> + Copy + Clone {
    any()
        // Use try_map over filter to get a better error on failure
        .try_map(|c: char, span| {
            if c.is_ascii_alphabetic() || c == '_' || c == '?' {
                Ok(c)
            } else {
                Err(Error::expected_found([], Some(MaybeRef::Val(c)), span))
            }
        })
        .then(
            any()
                // This error never appears due to `repeated` so can use `filter`
                .filter(|&c: &char| c.is_ascii_alphanumeric() || c == '_' || c == '?' || c == '\'')
                .repeated(),
        )
        .ignored()
        .or(just('~').ignored())
        .to_slice()
}

/// Like digits() but an exact number of then.
#[must_use]
pub fn n_digits<'a, C, I, E>(
    radix: u32,
    count: usize,
) -> Repeated<impl Parser<'a, I, C, E> + Copy + Clone, C, I, E>
where
    C: Char,
    I: ValueInput<'a> + Input<'a, Token = C>,
    E: ParserExtra<'a, I>,
{
    any()
        // Use try_map over filter to get a better error on failure
        .try_map(move |c: C, span| {
            if c.is_digit(radix) {
                Ok(c)
            } else {
                Err(Error::expected_found([], Some(MaybeRef::Val(c)), span))
            }
        })
        .repeated()
        .exactly(count)
}

pub fn lexer<'src>(
) -> impl Parser<'src, &'src str, Vec<(Token, Span)>, extra::Err<Rich<'src, char, Span>>> {
    // Arbitrary length positive integer.
    // Negative values are tokenized as `-` plus `Num(...)`.
    let num = text::digits(10)
        .to_slice()
        .map(|s: &str| Token::Num(s.to_owned()))
        .boxed();

    // Real number.
    let real = text::digits(10)
        .then(just('.'))
        .then(text::digits(10))
        .to_slice()
        .map(|s: &str| Token::Real(s.to_owned()))
        .boxed();

    // Hex number.
    let hex = just("0x")
        .ignore_then(
            any()
                .filter(|c: &char| c.is_ascii_hexdigit() || *c == '_')
                .repeated()
                .at_least(1),
        )
        .to_slice()
        .map(|s: &str| Token::Hex(s.to_owned()))
        .boxed();

    // Binary number.
    let bin = just("0b")
        .ignore_then(
            any()
                .filter(|c: &char| matches!(c, '0' | '1' | '_'))
                .repeated()
                .at_least(1),
        )
        .to_slice()
        .map(|s: &str| Token::Bin(s.to_owned()))
        .boxed();

    // Strings.
    let escape = just('\\')
        .ignore_then(choice((
            just('\\'),
            just('"'),
            just('\''),
            just('n').to('\n'),
            just('t').to('\t'),
            just('b').to('\x08'),
            just('r').to('\r'),
            just('\n').to(' '), // TODO: Handle this properly.
            // Upstream Sail: exactly 3 decimal digits (e.g. \067), no 'd' prefix.
            n_digits(10, 3).to_slice().try_map(|digits: &str, span| {
                char::from_u32(u32::from_str_radix(digits, 10).unwrap())
                    .ok_or_else(|| Rich::custom(span, "invalid decimal escape value"))
            }),
            just('x').ignore_then(n_digits(16, 2).to_slice().try_map(|digits: &str, span| {
                char::from_u32(u32::from_str_radix(&digits, 16).unwrap())
                    .ok_or_else(|| Rich::custom(span, "invalid hex unicode value"))
            })),
        )))
        .boxed();

    let multiline_string = just("\"\"\"")
        .ignore_then(
            any()
                .and_is(just("\"\"\"").not())
                .repeated()
                .collect::<String>(),
        )
        .then_ignore(just("\"\"\""))
        .map(Token::MultilineString)
        .boxed();

    let string = multiline_string
        .or(just('"')
            .ignore_then(none_of(&['\\', '"', '\n', '\r']).or(escape).repeated())
            .then_ignore(just('"'))
            .to_slice()
            .map(|s: &str| Token::String(s.to_owned())))
        .boxed();

    // The order of these is important, e.g. <= must come before < otherwise
    // <= will be parsed as <, =.
    // Have to split it into two choices because there's more than 26 and
    // they are different types.
    let op = choice((
        just("|}").to(Token::RightCurlyBar),
        just("|]").to(Token::RightSquareBar),
        just(">=").to(Token::GreaterThanOrEqualTo),
        just("=>").to(Token::FatRightArrow),
        just("==").to(Token::EqualTo),
        just("<=").to(Token::LessThanOrEqualTo),
        just("<->").to(Token::DoubleArrow),
        just(":=").to(Token::ColonEqual),
        just("<-").to(Token::LeftArrow),
        just("{|").to(Token::LeftCurlyBar),
        just("[|").to(Token::LeftSquareBar),
        just("()").to(Token::Unit),
        just("!=").to(Token::NotEqualTo),
        just("::").to(Token::Scope),
        just("->").to(Token::RightArrow),
    ))
    .or(choice((
        just('$').to(Token::Dollar),
        just('#').to(Token::Hash),
        just('|').to(Token::Or),
        just('>').to(Token::GreaterThan),
        just('=').to(Token::Equal),
        just('<').to(Token::LessThan),
        just('+').to(Token::Plus),
        just('^').to(Token::Caret),
        just('%').to(Token::Modulus),
        just('&').to(Token::And),
        just('/').to(Token::Divide),
        just('*').to(Token::Multiply),
        just('@').to(Token::At),
        just('}').to(Token::RightCurlyBracket),
        just('{').to(Token::LeftCurlyBracket),
        just(']').to(Token::RightSquareBracket),
        just('[').to(Token::LeftSquareBracket),
        just(')').to(Token::RightBracket),
        just('(').to(Token::LeftBracket),
        just('.').to(Token::Dot),
        just(':').to(Token::Colon),
        just(';').to(Token::Semicolon),
        just(',').to(Token::Comma),
        just('-').to(Token::Minus),
        just('_').to(Token::Underscore),
    )))
    .boxed();

    // TyVar
    let tyvar = just('\'')
        .ignore_then(ident())
        .to_slice()
        .map(|s: &str| Token::TyVal(s.to_owned()))
        .boxed();

    // Structured directives like `$foo{...}`. Consume the opening `{` so the parser can
    // parse the payload from subsequent tokens, including nested objects and lists.
    let structured_directive = just('$')
        .ignore_then(ident())
        .then_ignore(just('{'))
        .map(|name: &str| Token::StructuredDirectiveStart(name.to_string()))
        .boxed();

    // Sail directives like `$option ...`, `$start_module# C`, `$include_error ...`.
    // Keep a single token and consume directive payload until line end.
    let directive = just('$')
        .ignore_then(ident())
        .then(none_of('\n').repeated().to_slice().or_not())
        .map(|(name, payload): (&str, Option<&str>)| Token::Directive {
            name: name.to_string(),
            payload: payload.and_then(|payload| (!payload.is_empty()).then(|| payload.to_string())),
        })
        .boxed();

    // A parser for identifiers and keywords.
    // '~' is a specially allowed identifier.
    let ident = ident()
        .map(|ident: &str| match ident {
            "_" => Token::Underscore,
            "and" => Token::KwAnd,
            "as" => Token::KwAs,
            "assert" => Token::KwAssert,
            "backwards" => Token::KwBackwards,
            "barr" => Token::KwBarr,
            "bitfield" => Token::KwBitfield,
            "bitone" => Token::KwBitone,
            "bitzero" => Token::KwBitzero,
            "Bool" => Token::KwBool,
            "by" => Token::KwBy,
            "cast" => Token::KwCast,
            "catch" => Token::KwCatch,
            "case" => Token::KwCase,
            "clause" => Token::KwClause,
            "configuration" => Token::KwConfiguration,
            "constant" => Token::KwConstant,
            "constraint" => Token::KwConstraint,
            "dec" => Token::KwDec,
            "default" => Token::KwDefault,
            "depend" => Token::KwDepend,
            "do" => Token::KwDo,
            "downto" => Token::KwDownto,
            "eamem" => Token::KwEamem,
            "effect" => Token::KwEffect,
            "else" => Token::KwElse,
            "end" => Token::KwEnd,
            "enum" => Token::KwEnum,
            "escape" => Token::KwEscape,
            "exit" => Token::KwExit,
            "exmem" => Token::KwExmem,
            "false" => Token::KwFalse,
            "forall" => Token::KwForall,
            "foreach" => Token::KwForeach,
            "forwards" => Token::KwForwards,
            "from" => Token::KwFrom,
            "function" => Token::KwFunction,
            "if" => Token::KwIf,
            "impl" => Token::KwImpl,
            "in" => Token::KwIn,
            "inc" => Token::KwInc,
            "infix" => Token::KwInfix,
            "infixl" => Token::KwInfixl,
            "infixr" => Token::KwInfixr,
            "instantiation" => Token::KwInstantiation,
            "Int" => Token::KwInt,
            "let" => Token::KwLet,
            "mapping" => Token::KwMapping,
            "match" => Token::KwMatch,
            "monadic" => Token::KwMonadic,
            "mutual" => Token::KwMutual,
            "mwv" => Token::KwMwv,
            "newtype" => Token::KwNewtype,
            "nondet" => Token::KwNondet,
            "Order" => Token::KwOrder,
            "outcome" => Token::KwOutcome,
            "overload" => Token::KwOverload,
            "private" => Token::KwPrivate,
            "pure" => Token::KwPure,
            "ref" => Token::KwRef,
            "register" => Token::KwRegister,
            "repeat" => Token::KwRepeat,
            "return" => Token::KwReturn,
            "rmem" => Token::KwRmem,
            "rreg" => Token::KwRreg,
            "scattered" => Token::KwScattered,
            "sizeof" => Token::KwSizeof,
            "struct" => Token::KwStruct,
            "switch" => Token::KwSwitch,
            "termination_measure" => Token::KwTerminationMeasure,
            "then" => Token::KwThen,
            "throw" => Token::KwThrow,
            "to" => Token::KwTo,
            "true" => Token::KwTrue,
            "try" => Token::KwTry,
            "type" => Token::KwType,
            "Type" => Token::KwTypeUpper,
            "undef" => Token::KwUndef,
            "undefined" => Token::KwUndefined,
            "union" => Token::KwUnion,
            "unspec" => Token::KwUnspec,
            "until" => Token::KwUntil,
            "val" => Token::KwVal,
            "var" => Token::KwVar,
            "when" => Token::KwWhen,
            "while" => Token::KwWhile,
            "with" => Token::KwWith,
            "wmem" => Token::KwWmem,
            "wreg" => Token::KwWreg,
            _ => Token::Id(ident.to_string()),
        })
        .boxed();

    // A single token can be one of the above
    let token = choice((
        structured_directive,
        directive,
        tyvar,
        hex,
        bin,
        real,
        num,
        string,
        ident,
        op,
    ))
    .recover_with(skip_then_retry_until(any().ignored(), end()))
    .boxed();

    let line_comment = just("//").then(none_of('\n').repeated()).padded().ignored();
    let block_comment = just("/*")
        .then(any().and_is(just("*/").not()).repeated())
        .then(just("*/"))
        .padded()
        .ignored();
    let ml_comment = just("(*")
        .then(any().and_is(just("*)").not()).repeated())
        .then(just("*)"))
        .padded()
        .ignored();

    let comment = line_comment.or(block_comment).or(ml_comment);

    token
        .map_with(|tok, e| (tok, e.span()))
        .padded_by(comment.repeated())
        .padded()
        .repeated()
        .collect()
        .then_ignore(end())
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_basic() {
        let code = r#"
/* This is a slightly arbitrary limit on the maximum number of bytes
   in a memory access.  It helps to generate slightly better C code
   because it means width argument can be fast native integer. It
   would be even better if it could be <= 8 bytes so that data can
   also be a 64-bit int but CHERI needs 128-bit accesses for
   capabilities and SIMD / vector instructions will also need more. */
type max_mem_access : Int = 16

val write_ram = {lem: "write_ram", coq: "write_ram"} : forall 'n, 0 < 'n <= max_mem_access . (write_kind, xlenbits, atom('n), bits(8 * 'n), mem_meta) -> bool effect {wmv, wmvt}
function write_ram(wk, addr, width, data, meta) = {
  /* Write out metadata only if the value write succeeds.
   * It is assumed for now that this write always succeeds;
   * there is currently no return value.
   * FIXME: We should convert the external API for all backends
   * (not just for Lem) to consume the value along with the
   * metadata to ensure atomicity.
   */
  let ret : bool = __write_mem(wk, sizeof(xlen), addr, width, data);
  if ret then __WriteRAM_Meta(addr, width, meta);
  ret
}

val write_ram_ea : forall 'n, 0 < 'n <= max_mem_access . (write_kind, xlenbits, atom('n)) -> unit effect {eamem}
function write_ram_ea(wk, addr, width) =
  __write_mem_ea(wk, sizeof(xlen), addr, width)

val read_ram = {lem: "read_ram", coq: "read_ram"} : forall 'n, 0 < 'n <= max_mem_access .  (read_kind, xlenbits, atom('n), bool) -> (bits(8 * 'n), mem_meta) effect {rmem, rmemt}
function read_ram(rk, addr, width, read_meta) =
  let meta = if read_meta then __ReadRAM_Meta(addr, width) else default_meta in
  (__read_mem(rk, sizeof(xlen), addr, width), meta)

val __TraceMemoryWrite : forall 'n 'm. (atom('n), bits('m), bits(8 * 'n)) -> unit
val __TraceMemoryRead  : forall 'n 'm. (atom('n), bits('m), bits(8 * 'n)) -> unit
"#;
        let result = lexer().parse(code);
        assert!(result.output().is_some());
        assert!(result.errors().peekable().peek().is_none());
    }

    #[test]
    fn test_span_bytes() {
        // Check that the span is in bytes and works with unicode characters.
        let code = "/* 😊 */ foo";
        let result = lexer().parse(code);
        assert!(result.output().is_some());
        assert!(result.errors().peekable().peek().is_none());
    }

    #[test]
    fn accepts_start_module_directive_hash() {
        let code = "$start_module# C\nval x : int";
        let result = lexer().parse(code);
        let errors: Vec<_> = result.errors().collect();
        assert!(result.output().is_some(), "lexer output should exist");
        assert!(errors.is_empty(), "unexpected lexer errors: {:?}", errors);
        let tokens = result.output().expect("tokens");
        assert!(matches!(
            &tokens[0].0,
            Token::Directive { name, payload }
                if name == "start_module" && payload.as_deref() == Some("# C")
        ));
    }

    #[test]
    fn accepts_structured_directive_start() {
        let code = "$pragma{enabled = true}\nval x : int";
        let result = lexer().parse(code);
        let errors: Vec<_> = result.errors().collect();
        assert!(result.output().is_some(), "lexer output should exist");
        assert!(errors.is_empty(), "unexpected lexer errors: {:?}", errors);
        let tokens = result.output().expect("tokens");
        assert!(matches!(
            &tokens[0].0,
            Token::StructuredDirectiveStart(name) if name == "pragma"
        ));
        assert!(matches!(&tokens[1].0, Token::Id(name) if name == "enabled"));
    }

    #[test]
    fn accepts_underscored_binary_and_hex_literals() {
        let code = "let x = 0xFFFF_FFFF\nlet y = 0b1010_0101";
        let result = lexer().parse(code);
        let errors: Vec<_> = result.errors().collect();
        assert!(result.output().is_some(), "lexer output should exist");
        assert!(errors.is_empty(), "unexpected lexer errors: {:?}", errors);
        let tokens = result.output().expect("tokens");
        assert!(tokens
            .iter()
            .any(|(token, _)| matches!(token, Token::Hex(text) if text == "0xFFFF_FFFF")));
        assert!(tokens
            .iter()
            .any(|(token, _)| matches!(token, Token::Bin(text) if text == "0b1010_0101")));
    }

    #[test]
    fn accepts_switch_case_and_colon_equal_tokens() {
        let code = "switch x { case y -> z := ~(y) }";
        let result = lexer().parse(code);
        let errors: Vec<_> = result.errors().collect();
        assert!(result.output().is_some(), "lexer output should exist");
        assert!(errors.is_empty(), "unexpected lexer errors: {:?}", errors);
        let tokens = result.output().expect("tokens");
        assert!(tokens.iter().any(|(token, _)| *token == Token::KwSwitch));
        assert!(tokens.iter().any(|(token, _)| *token == Token::KwCase));
        assert!(tokens.iter().any(|(token, _)| *token == Token::ColonEqual));
    }

    #[test]
    fn accepts_option_flag_with_double_minus() {
        let code = "$option --dallow-internal\nval x : int";
        let result = lexer().parse(code);
        let errors: Vec<_> = result.errors().collect();
        assert!(result.output().is_some(), "lexer output should exist");
        assert!(errors.is_empty(), "unexpected lexer errors: {:?}", errors);
    }

    #[test]
    fn accepts_include_error_payload_with_backticks() {
        let code = "$include_error A default order must be set (using `default Order dec` or `default Order inc`) before including this file\nval x : int";
        let result = lexer().parse(code);
        let errors: Vec<_> = result.errors().collect();
        assert!(result.output().is_some(), "lexer output should exist");
        assert!(errors.is_empty(), "unexpected lexer errors: {:?}", errors);
        let tokens = result.output().expect("tokens");
        assert!(matches!(
            &tokens[0].0,
            Token::Directive { name, payload }
                if name == "include_error"
                    && payload
                        .as_deref()
                        .map(|payload| payload.contains("default Order dec"))
                        .unwrap_or(false)
        ));
    }

    #[test]
    fn accepts_bang_followed_by_space() {
        let code =
            "if m then (rfp::ars) else ars) (* in memory case r is a third input to address! *)";
        let result = lexer().parse(code);
        let errors: Vec<_> = result.errors().collect();
        assert!(result.output().is_some(), "lexer output should exist");
        assert!(errors.is_empty(), "unexpected lexer errors: {:?}", errors);
    }

    #[test]
    fn reports_unclosed_doc_comment_prefix() {
        let code = "/*! doc";
        let result = lexer().parse(code);
        let errors: Vec<_> = result.errors().collect();
        assert!(
            !errors.is_empty(),
            "expected lexer errors for doc comment EOF"
        );
    }

    #[test]
    fn reports_unicode_tm_identifier() {
        let code = "let _ = ™;";
        let result = lexer().parse(code);
        let errors: Vec<_> = result.errors().collect();
        assert!(
            !errors.is_empty(),
            "expected lexer errors for extended ascii"
        );
    }

    #[test]
    fn reports_formfeed_escape_in_string() {
        let code = "let x = \"\\f\"";
        let result = lexer().parse(code);
        let errors: Vec<_> = result.errors().collect();
        assert!(
            !errors.is_empty(),
            "expected lexer errors for illegal escape"
        );
    }

    #[test]
    fn reports_unterminated_string_at_line_end() {
        let code = "let x = \"\n}";
        let result = lexer().parse(code);
        let errors: Vec<_> = result.errors().collect();
        assert!(
            !errors.is_empty(),
            "expected lexer errors for unterminated string"
        );
    }
}
