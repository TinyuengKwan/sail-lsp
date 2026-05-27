//! Generated token wrappers from `sail.ungram`.
//!
//! Each token type wraps a `SyntaxToken` and implements `AstToken`.

use parser::SyntaxKind;

use crate::syntax_node::SyntaxToken;
use crate::ast::AstToken;

macro_rules! ast_token {
    ($name:ident, $kind:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name {
            pub(crate) syntax: SyntaxToken,
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                std::fmt::Display::fmt(&self.syntax, f)
            }
        }

        impl AstToken for $name {
            fn can_cast(kind: SyntaxKind) -> bool {
                kind == SyntaxKind::$kind
            }
            fn cast(syntax: SyntaxToken) -> Option<Self> {
                if Self::can_cast(syntax.kind()) {
                    Some(Self { syntax })
                } else {
                    None
                }
            }
            fn syntax(&self) -> &SyntaxToken {
                &self.syntax
            }
        }
    };
}

// Identifiers
ast_token!(Ident, IDENT);
ast_token!(TyVar, TY_VAR);

// Literals
ast_token!(BinLit, BIN_LIT);
ast_token!(HexLit, HEX_LIT);
ast_token!(NumLit, NUM_LIT);
ast_token!(RealLit, REAL_LIT);
ast_token!(StringLit, STRING_LIT);
ast_token!(MultilineStringLit, MULTILINE_STRING_LIT);

// Comments
ast_token!(LineComment, LINE_COMMENT);
ast_token!(BlockComment, BLOCK_COMMENT);
ast_token!(DocComment, DOC_COMMENT);

// Whitespace
ast_token!(Whitespace, WHITESPACE);
