//! What a highlighted range *is*, in the IDE's own words.
//!
//! rust-analyzer's `ide/src/syntax_highlighting/tags.rs`: a `Highlight` is a
//! tag and a set of modifiers, and the protocol legend — which index means
//! which word, which words are standard and which are this server's own —
//! lives in the binary crate (`lsp/semantic_tokens.rs`). Here the tag used
//! to be a `u32` into a legend declared in this module, so adding a token
//! type meant renumbering comments, and the server could not tell a string
//! from a register without knowing the numbers.

use std::ops;

/// The kind of a highlighted range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HlTag {
    Keyword,
    Function,
    Type,
    Enum,
    Variable,
    TypeParameter,
    StringLiteral,
    NumericLiteral,
    Operator,
    Struct,
    EnumMember,
    Property,
    Comment,
    Register,
    Parameter,
}

/// A modifier on a highlighted range. The discriminant is the bit it
/// occupies in [`HlMods`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum HlMod {
    /// The range declares its name.
    Declaration = 0,
    /// The range declares *and* defines its name — not a `val` signature.
    Definition,
    /// A `let` binding.
    Readonly,
    /// A `var` binding, or a write.
    Modification,
    /// A doc comment.
    Documentation,
    /// A register: process-wide state.
    Static,
    /// `if`, `match`, `return`, ...
    ControlFlow,
}

impl HlMod {
    pub const ALL: &'static [HlMod] = &[
        HlMod::Declaration,
        HlMod::Definition,
        HlMod::Readonly,
        HlMod::Modification,
        HlMod::Documentation,
        HlMod::Static,
        HlMod::ControlFlow,
    ];

    fn mask(self) -> u32 {
        1 << (self as u32)
    }
}

/// A set of [`HlMod`]s.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct HlMods(u32);

impl HlMods {
    pub fn contains(self, m: HlMod) -> bool {
        self.0 & m.mask() != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn iter(self) -> impl Iterator<Item = HlMod> {
        HlMod::ALL.iter().copied().filter(move |m| self.contains(*m))
    }
}

/// A tag and its modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Highlight {
    pub tag: HlTag,
    pub mods: HlMods,
}

impl Highlight {
    pub fn new(tag: HlTag) -> Self {
        Self { tag, mods: HlMods::default() }
    }
}

impl From<HlTag> for Highlight {
    fn from(tag: HlTag) -> Self {
        Highlight::new(tag)
    }
}

impl ops::BitOr<HlMod> for HlTag {
    type Output = Highlight;
    fn bitor(self, m: HlMod) -> Highlight {
        Highlight::new(self) | m
    }
}

impl ops::BitOr<HlMod> for Highlight {
    type Output = Highlight;
    fn bitor(mut self, m: HlMod) -> Highlight {
        self |= m;
        self
    }
}

impl ops::BitOrAssign<HlMod> for Highlight {
    fn bitor_assign(&mut self, m: HlMod) {
        self.mods.0 |= m.mask();
    }
}

impl ops::BitOrAssign<HlMods> for Highlight {
    fn bitor_assign(&mut self, m: HlMods) {
        self.mods.0 |= m.0;
    }
}

/// The tag of a lexer token that is not a name — a keyword, a literal, an
/// operator — and of a name, by the keyword that precedes it, when nothing
/// better is known about it.
pub(super) fn token_tag(token: &parser::Token, previous: Option<&parser::Token>) -> Option<HlTag> {
    let tag = match token {
        // NOTE: Comments are skipped by the lexer (hand_lexer.rs:28-55)
        // and don't appear as tokens. Comment highlighting is handled by
        // the editor's built-in TextMate grammar.
        parser::Token::Id(_) => match previous {
            Some(parser::Token::KwFunction)
            | Some(parser::Token::KwVal)
            | Some(parser::Token::KwMapping)
            | Some(parser::Token::KwOverload) => HlTag::Function,
            Some(parser::Token::KwType) | Some(parser::Token::KwUnion) => HlTag::Type,
            Some(parser::Token::KwStruct) | Some(parser::Token::KwBitfield) => HlTag::Struct,
            Some(parser::Token::KwEnum) => HlTag::Enum,
            Some(parser::Token::KwRegister) => HlTag::Register,
            Some(parser::Token::KwLet) | Some(parser::Token::KwVar) => HlTag::Variable,
            _ => HlTag::Variable,
        },
        parser::Token::TyVal(_) => HlTag::TypeParameter,
        parser::Token::String(_) => HlTag::StringLiteral,
        parser::Token::Num(_)
        | parser::Token::Real(_)
        | parser::Token::Bin(_)
        | parser::Token::Hex(_) => HlTag::NumericLiteral,
        _ if token_is_keyword(token) => HlTag::Keyword,
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
            HlTag::Operator
        }
        _ => return None,
    };
    Some(tag)
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
