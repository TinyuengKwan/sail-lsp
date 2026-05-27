//! Generated from `sail.ungram`, do not edit by hand.
//!
//! Run `cargo test -p syntax -- codegen` to validate.

/// Every distinct syntactic element in a Sail source file.
/// Token-level kinds come first, then composite (node) kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
#[allow(non_camel_case_types, dead_code)]
pub enum SyntaxKind {

    // Identifiers
    IDENT = 0,
    TY_VAR,

    // Literals
    BIN_LIT,
    HEX_LIT,
    NUM_LIT,
    REAL_LIT,
    STRING_LIT,
    MULTILINE_STRING_LIT,

    // Punctuation
    DOLLAR,
    HASH,
    L_PAREN,        // (
    R_PAREN,        // )
    L_BRACK,      // [
    R_BRACK,      // ]
    L_CURLY,        // {
    R_CURLY,        // }
    R_ARROW,        // ->
    L_ARROW,        // <-
    FAT_R_ARROW,    // =>
    DOUBLE_ARROW,   // <->
    COLON_EQ,       // :=
    COMMA,
    COLON,
    SEMICOLON,
    DOT,
    CARET,          // ^
    AT,             // @
    L_ANGLE,        // <
    R_ANGLE,        // >
    LE,             // <=
    GE,             // >=
    PERCENT,        // %
    STAR,           // *
    SLASH,          // /
    EQ,             // =
    EQ_EQ,          // ==
    NEQ,            // !=
    AMP,            // &
    PIPE,           // |
    SCOPE,          // ::
    PLUS,
    MINUS,
    L_CURLY_BAR,    // {|
    R_CURLY_BAR,    // |}
    L_BRACKET_BAR,  // [|
    R_BRACKET_BAR,  // |]
    UNDERSCORE, // _
    UNIT,       // ()

    /// Infix operator with subscript (e.g. `<_s`, `<=_u`).
    /// Upstream lexer.mll:186-189: operatorn with `_ident` suffix.
    OP_IDENT,

    // Directives
    DIRECTIVE,
    STRUCTURED_DIRECTIVE_START,

    // Keywords
    KW_AND,          // GRAMMAR(sail): upstream token And (lexer.mll)  Confidence: HIGH
    KW_AS,           // GRAMMAR(sail): upstream token As (lexer.mll)  Confidence: HIGH
    KW_ASSERT,       // GRAMMAR(sail): upstream token Assert (lexer.mll)  Confidence: HIGH
    KW_BACKWARDS,    // GRAMMAR(sail): upstream token Backwards (lexer.mll)  Confidence: HIGH
    KW_BARR,         // GRAMMAR(sail): CUSTOM(sail-lsp) — no dedicated upstream token  Confidence: HIGH
    KW_BITFIELD,     // GRAMMAR(sail): upstream token Bitfield (lexer.mll)  Confidence: HIGH
    KW_BITONE,       // GRAMMAR(sail): upstream token Bitone (lexer.mll)  Confidence: HIGH
    KW_BITZERO,      // GRAMMAR(sail): upstream token Bitzero (lexer.mll)  Confidence: HIGH
    KW_BOOL,         // GRAMMAR(sail): upstream token BOOL (lexer.mll)  Confidence: HIGH
    KW_BY,           // GRAMMAR(sail): upstream token By (lexer.mll)  Confidence: HIGH
    KW_CAST,         // GRAMMAR(sail): upstream token Cast (lexer.mll)  Confidence: HIGH
    KW_CATCH,        // GRAMMAR(sail): upstream token Catch (lexer.mll)  Confidence: HIGH
    KW_CASE,
    KW_CLAUSE,       // GRAMMAR(sail): upstream token Clause (lexer.mll)  Confidence: HIGH
    KW_CONFIG,       // GRAMMAR(sail): upstream token Config (lexer.mll)  Confidence: HIGH
    KW_CONFIGURATION, // GRAMMAR(sail): upstream token Configuration (lexer.mll)  Confidence: HIGH
    KW_CONSTANT,     // GRAMMAR(sail): upstream token Constant (lexer.mll)  Confidence: HIGH
    KW_CONSTRAINT,   // GRAMMAR(sail): upstream token Constraint (lexer.mll)  Confidence: HIGH
    KW_DEC,          // GRAMMAR(sail): upstream token Dec (lexer.mll)  Confidence: HIGH
    KW_DEFAULT,      // GRAMMAR(sail): upstream token Default (lexer.mll)  Confidence: HIGH
    KW_DEPEND,       // GRAMMAR(sail): CUSTOM(sail-lsp) — no dedicated upstream token  Confidence: HIGH
    KW_DO,           // GRAMMAR(sail): upstream token Do (lexer.mll)  Confidence: HIGH
    KW_DOWNTO,       // GRAMMAR(sail): upstream token Downto (lexer.mll)  Confidence: HIGH
    KW_EAMEM,        // GRAMMAR(sail): CUSTOM(sail-lsp) — no dedicated upstream token  Confidence: HIGH
    KW_EFFECT,       // GRAMMAR(sail): upstream token Effect (lexer.mll)  Confidence: HIGH
    KW_ELSE,         // GRAMMAR(sail): upstream token Else (lexer.mll)  Confidence: HIGH
    KW_END,          // GRAMMAR(sail): upstream token End (lexer.mll)  Confidence: HIGH
    KW_ENUM,         // GRAMMAR(sail): upstream token Enum (lexer.mll)  Confidence: HIGH
    KW_ESCAPE,       // GRAMMAR(sail): CUSTOM(sail-lsp) — no dedicated upstream token  Confidence: HIGH
    KW_EXIT,         // GRAMMAR(sail): upstream token Exit (lexer.mll)  Confidence: HIGH
    KW_EXMEM,        // GRAMMAR(sail): CUSTOM(sail-lsp) — no dedicated upstream token  Confidence: HIGH
    KW_FALSE,        // GRAMMAR(sail): upstream token False (lexer.mll)  Confidence: HIGH
    KW_FORALL,       // GRAMMAR(sail): upstream token Forall (lexer.mll)  Confidence: HIGH
    KW_FOREACH,      // GRAMMAR(sail): upstream token Foreach (lexer.mll)  Confidence: HIGH
    KW_FORWARDS,     // GRAMMAR(sail): upstream token Forwards (lexer.mll)  Confidence: HIGH
    KW_FROM,         // GRAMMAR(sail): upstream token From (lexer.mll)  Confidence: HIGH
    KW_FUNCTION,     // GRAMMAR(sail): upstream token Function_ (lexer.mll)  Confidence: HIGH
    KW_IF,           // GRAMMAR(sail): upstream token If_ (lexer.mll)  Confidence: HIGH
    KW_IMPL,         // GRAMMAR(sail): upstream token Impl (lexer.mll)  Confidence: HIGH
    KW_IN,           // GRAMMAR(sail): upstream token In (lexer.mll)  Confidence: HIGH
    KW_INC,          // GRAMMAR(sail): upstream token Inc (lexer.mll)  Confidence: HIGH
    KW_INFIX,
    KW_INFIXL,
    KW_INFIXR,
    KW_INSTANTIATION, // GRAMMAR(sail): upstream token Instantiation (lexer.mll)  Confidence: HIGH
    KW_INT,          // GRAMMAR(sail): upstream token INT (lexer.mll)  Confidence: HIGH
    KW_LET,          // GRAMMAR(sail): upstream token Let_ (lexer.mll)  Confidence: HIGH
    KW_MAPPING,      // GRAMMAR(sail): upstream token Mapping (lexer.mll)  Confidence: HIGH
    KW_MATCH,        // GRAMMAR(sail): upstream token Match (lexer.mll)  Confidence: HIGH
    KW_MONADIC,      // GRAMMAR(sail): upstream token Monadic (lexer.mll)  Confidence: HIGH
    KW_MUTUAL,       // GRAMMAR(sail): upstream token Mutual (lexer.mll)  Confidence: HIGH
    KW_MWV,          // GRAMMAR(sail): CUSTOM(sail-lsp) — no dedicated upstream token  Confidence: HIGH
    KW_NEWTYPE,      // GRAMMAR(sail): upstream token Newtype (lexer.mll)  Confidence: HIGH
    KW_NONDET,       // GRAMMAR(sail): CUSTOM(sail-lsp) — no dedicated upstream token  Confidence: HIGH
    KW_ORDER,        // GRAMMAR(sail): upstream token ORDER (lexer.mll)  Confidence: HIGH
    KW_OUTCOME,      // GRAMMAR(sail): upstream token Outcome (lexer.mll)  Confidence: HIGH
    KW_OVERLOAD,     // GRAMMAR(sail): upstream token Overload (lexer.mll)  Confidence: HIGH
    KW_PRIVATE,      // GRAMMAR(sail): upstream token Private (lexer.mll)  Confidence: HIGH
    KW_PURE,         // GRAMMAR(sail): upstream token Pure (lexer.mll)  Confidence: HIGH
    KW_REF,          // GRAMMAR(sail): upstream token Ref (lexer.mll)  Confidence: HIGH
    KW_REGISTER,     // GRAMMAR(sail): upstream token Register (lexer.mll)  Confidence: HIGH
    KW_REPEAT,       // GRAMMAR(sail): upstream token Repeat (lexer.mll)  Confidence: HIGH
    KW_RETURN,       // GRAMMAR(sail): upstream token Return (lexer.mll)  Confidence: HIGH
    KW_RMEM,         // GRAMMAR(sail): CUSTOM(sail-lsp) — no dedicated upstream token  Confidence: HIGH
    KW_RREG,         // GRAMMAR(sail): CUSTOM(sail-lsp) — no dedicated upstream token  Confidence: HIGH
    KW_SCATTERED,    // GRAMMAR(sail): upstream token Scattered (lexer.mll)  Confidence: HIGH
    KW_SIZEOF,       // GRAMMAR(sail): upstream token Sizeof (lexer.mll)  Confidence: HIGH
    KW_STRUCT,       // GRAMMAR(sail): upstream token Struct (lexer.mll)  Confidence: HIGH
    KW_SWITCH,
    KW_TERMINATION_MEASURE,
    KW_THEN,         // GRAMMAR(sail): upstream token Then (lexer.mll)  Confidence: HIGH
    KW_THROW,        // GRAMMAR(sail): upstream token Throw (lexer.mll)  Confidence: HIGH
    KW_TO,           // GRAMMAR(sail): upstream token To (lexer.mll)  Confidence: HIGH
    KW_TRUE,         // GRAMMAR(sail): upstream token True (lexer.mll)  Confidence: HIGH
    KW_TRY,          // GRAMMAR(sail): upstream token Try (lexer.mll)  Confidence: HIGH
    KW_TYPE,         // GRAMMAR(sail): upstream token Typedef (lexer.mll)  Confidence: HIGH
    KW_TYPE_UPPER,   // GRAMMAR(sail): upstream token TYPE (lexer.mll)  Confidence: HIGH
    KW_UNDEF,
    KW_UNDEFINED,    // GRAMMAR(sail): upstream token Undefined (lexer.mll)  Confidence: HIGH
    KW_UNION,        // GRAMMAR(sail): upstream token Union (lexer.mll)  Confidence: HIGH
    KW_UNSPEC,       // GRAMMAR(sail): CUSTOM(sail-lsp) — no dedicated upstream token  Confidence: HIGH
    KW_UNTIL,        // GRAMMAR(sail): upstream token Until (lexer.mll)  Confidence: HIGH
    KW_VAL,          // GRAMMAR(sail): upstream token Val (lexer.mll)  Confidence: HIGH
    KW_VAR,          // GRAMMAR(sail): upstream token Var (lexer.mll)  Confidence: HIGH
    KW_WHEN,         // GRAMMAR(sail): upstream token When (lexer.mll)  Confidence: HIGH
    KW_WHILE,        // GRAMMAR(sail): upstream token While (lexer.mll)  Confidence: HIGH
    KW_WITH,         // GRAMMAR(sail): upstream token With (lexer.mll)  Confidence: HIGH
    KW_WMEM,         // GRAMMAR(sail): CUSTOM(sail-lsp) — no dedicated upstream token  Confidence: HIGH
    KW_WREG,         // GRAMMAR(sail): CUSTOM(sail-lsp) — no dedicated upstream token  Confidence: HIGH

    // Trivia (whitespace + comments, preserved in full-fidelity mode)
    WHITESPACE,
    LINE_COMMENT,
    BLOCK_COMMENT,
    /// `///` doc comment — preserved as non-trivia.
    DOC_COMMENT,

    // Error token
    ERROR,

    // --- BEGIN GENERATED COMPOSITE KINDS ---

    // Top-level
    CALLABLE_DEF,
    CALLABLE_SPEC,
    CONSTRAINT_DEF,
    DEFAULT_DEF,
    DEFINITION,
    DIRECTIVE_DEF,
    END_DEF,
    FIXITY_DEF,
    INSTANTIATION_DEF,
    NAMED_DEF,
    OUTCOME_DEF,
    SCATTERED_CLAUSE_DEF,
    SCATTERED_DEF,
    SOURCE_FILE,
    TERMINATION_MEASURE_DEF,
    TYPE_ALIAS_DEF,

    // Expressions
    ASSERT_EXPR,
    ASSIGN_EXPR,
    BIN_EXPR,
    BLOCK_EXPR,
    CALL_EXPR,
    CAST_EXPR,
    CONFIG_EXPR,
    CONSTRAINT_EXPR,
    EXIT_EXPR,
    FIELD_ACCESS_EXPR,
    FOREACH_EXPR,
    IDENT_EXPR,
    IF_EXPR,
    INDEX_EXPR,
    LET_EXPR,
    LIST_EXPR,
    LITERAL_EXPR,
    MATCH_EXPR,
    PREFIX_EXPR,
    REF_EXPR,
    REPEAT_EXPR,
    RETURN_EXPR,
    SIZEOF_EXPR,
    STRUCT_EXPR,
    SUBRANGE_EXPR,
    THROW_EXPR,
    TRY_EXPR,
    TUPLE_EXPR,
    TYVAR_EXPR,
    UPDATE_EXPR,
    VAR_EXPR,
    VECTOR_EXPR,
    VECTOR_UPDATE_EXPR,
    WHILE_EXPR,

    // Patterns
    APP_PAT,
    AS_PAT,
    BIN_PAT,
    IDENT_PAT,
    INDEX_PAT,
    LIST_PAT,
    LITERAL_PAT,
    RANGE_INDEX_PAT,
    STRUCT_PAT,
    TUPLE_PAT,
    TYPED_PAT,
    TYVAR_PAT,
    VECTOR_PAT,
    WILD_PAT,

    // Type expressions
    TYPE_APP,
    TYPE_ARROW,
    TYPE_EFFECT,
    TYPE_EXISTENTIAL,
    TYPE_FORALL,
    TYPE_NAMED,
    TYPE_PARAM_LIST,
    TYPE_TUPLE,
    TYPE_VAR,

    // Sub-structures
    ARG_LIST,
    ATTRIBUTE,
    BLOCK_ITEM,
    BODY,
    FIELD_INIT,
    MATCH_ARM,
    NAME,
    PARAM_LIST,
    QUANTIFIER,
    VISIBILITY,

    // --- END GENERATED COMPOSITE KINDS ---

    // Internal sentinels — never emitted to rowan
    /// End-of-file sentinel for parser dispatch.
    EOF,
    /// Placeholder for abandoned markers in the event stream.
    TOMBSTONE,

    /// Sentinel — must be last. Used for range checks.
    #[doc(hidden)]
    __LAST,
}

/// Token shorthand macro.
///
/// Usage: `T![function]`, `T![+]`, `T!['(']`, etc.
#[macro_export]
macro_rules! T_ {
    // Keywords
    [and] => { $crate::SyntaxKind::KW_AND };
    [as] => { $crate::SyntaxKind::KW_AS };
    [assert] => { $crate::SyntaxKind::KW_ASSERT };
    [backwards] => { $crate::SyntaxKind::KW_BACKWARDS };
    [bitfield] => { $crate::SyntaxKind::KW_BITFIELD };
    [bool] => { $crate::SyntaxKind::KW_BOOL };
    [by] => { $crate::SyntaxKind::KW_BY };
    [cast] => { $crate::SyntaxKind::KW_CAST };
    [catch] => { $crate::SyntaxKind::KW_CATCH };
    [clause] => { $crate::SyntaxKind::KW_CLAUSE };
    [constraint] => { $crate::SyntaxKind::KW_CONSTRAINT };
    [dec] => { $crate::SyntaxKind::KW_DEC };
    [default] => { $crate::SyntaxKind::KW_DEFAULT };
    [do] => { $crate::SyntaxKind::KW_DO };
    [downto] => { $crate::SyntaxKind::KW_DOWNTO };
    [effect] => { $crate::SyntaxKind::KW_EFFECT };
    [else] => { $crate::SyntaxKind::KW_ELSE };
    [end] => { $crate::SyntaxKind::KW_END };
    [enum] => { $crate::SyntaxKind::KW_ENUM };
    [exit] => { $crate::SyntaxKind::KW_EXIT };
    [false] => { $crate::SyntaxKind::KW_FALSE };
    [forall] => { $crate::SyntaxKind::KW_FORALL };
    [foreach] => { $crate::SyntaxKind::KW_FOREACH };
    [forwards] => { $crate::SyntaxKind::KW_FORWARDS };
    [from] => { $crate::SyntaxKind::KW_FROM };
    [function] => { $crate::SyntaxKind::KW_FUNCTION };
    [if] => { $crate::SyntaxKind::KW_IF };
    [in] => { $crate::SyntaxKind::KW_IN };
    [inc] => { $crate::SyntaxKind::KW_INC };
    [infix] => { $crate::SyntaxKind::KW_INFIX };
    [infixl] => { $crate::SyntaxKind::KW_INFIXL };
    [infixr] => { $crate::SyntaxKind::KW_INFIXR };
    [int] => { $crate::SyntaxKind::KW_INT };
    [let] => { $crate::SyntaxKind::KW_LET };
    [mapping] => { $crate::SyntaxKind::KW_MAPPING };
    [match] => { $crate::SyntaxKind::KW_MATCH };
    [mutual] => { $crate::SyntaxKind::KW_MUTUAL };
    [newtype] => { $crate::SyntaxKind::KW_NEWTYPE };
    [order] => { $crate::SyntaxKind::KW_ORDER };
    [outcome] => { $crate::SyntaxKind::KW_OUTCOME };
    [overload] => { $crate::SyntaxKind::KW_OVERLOAD };
    [private] => { $crate::SyntaxKind::KW_PRIVATE };
    [pure] => { $crate::SyntaxKind::KW_PURE };
    [ref] => { $crate::SyntaxKind::KW_REF };
    [register] => { $crate::SyntaxKind::KW_REGISTER };
    [repeat] => { $crate::SyntaxKind::KW_REPEAT };
    [return] => { $crate::SyntaxKind::KW_RETURN };
    [scattered] => { $crate::SyntaxKind::KW_SCATTERED };
    [sizeof] => { $crate::SyntaxKind::KW_SIZEOF };
    [struct] => { $crate::SyntaxKind::KW_STRUCT };
    [switch] => { $crate::SyntaxKind::KW_SWITCH };
    [then] => { $crate::SyntaxKind::KW_THEN };
    [throw] => { $crate::SyntaxKind::KW_THROW };
    [to] => { $crate::SyntaxKind::KW_TO };
    [true] => { $crate::SyntaxKind::KW_TRUE };
    [try] => { $crate::SyntaxKind::KW_TRY };
    [type] => { $crate::SyntaxKind::KW_TYPE };
    [undefined] => { $crate::SyntaxKind::KW_UNDEFINED };
    [union] => { $crate::SyntaxKind::KW_UNION };
    [until] => { $crate::SyntaxKind::KW_UNTIL };
    [val] => { $crate::SyntaxKind::KW_VAL };
    [var] => { $crate::SyntaxKind::KW_VAR };
    [when] => { $crate::SyntaxKind::KW_WHEN };
    [while] => { $crate::SyntaxKind::KW_WHILE };
    [with] => { $crate::SyntaxKind::KW_WITH };

    // Punctuation
    ['('] => { $crate::SyntaxKind::L_PAREN };
    [')'] => { $crate::SyntaxKind::R_PAREN };
    ['['] => { $crate::SyntaxKind::L_BRACK };
    [']'] => { $crate::SyntaxKind::R_BRACK };
    ['{'] => { $crate::SyntaxKind::L_CURLY };
    ['}'] => { $crate::SyntaxKind::R_CURLY };
    [<] => { $crate::SyntaxKind::L_ANGLE };
    [>] => { $crate::SyntaxKind::R_ANGLE };
    [->] => { $crate::SyntaxKind::R_ARROW };
    [<-] => { $crate::SyntaxKind::L_ARROW };
    [=>] => { $crate::SyntaxKind::FAT_R_ARROW };
    [<->] => { $crate::SyntaxKind::DOUBLE_ARROW };
    [:=] => { $crate::SyntaxKind::COLON_EQ };
    [,] => { $crate::SyntaxKind::COMMA };
    [:] => { $crate::SyntaxKind::COLON };
    [;] => { $crate::SyntaxKind::SEMICOLON };
    [.] => { $crate::SyntaxKind::DOT };
    [^] => { $crate::SyntaxKind::CARET };
    [@] => { $crate::SyntaxKind::AT };
    [<=] => { $crate::SyntaxKind::LE };
    [>=] => { $crate::SyntaxKind::GE };
    [%] => { $crate::SyntaxKind::PERCENT };
    [*] => { $crate::SyntaxKind::STAR };
    [/] => { $crate::SyntaxKind::SLASH };
    [=] => { $crate::SyntaxKind::EQ };
    [==] => { $crate::SyntaxKind::EQ_EQ };
    [!=] => { $crate::SyntaxKind::NEQ };
    [&] => { $crate::SyntaxKind::AMP };
    [|] => { $crate::SyntaxKind::PIPE };
    [::] => { $crate::SyntaxKind::SCOPE };
    [+] => { $crate::SyntaxKind::PLUS };
    [-] => { $crate::SyntaxKind::MINUS };
    [_] => { $crate::SyntaxKind::UNDERSCORE };
}
