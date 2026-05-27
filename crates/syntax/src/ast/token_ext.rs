//! Extension methods for tokens.
//! Provides semantic classification of comment and string tokens.

use parser::SyntaxKind as SK;

/// Classification of comment tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CommentKind {
    pub shape: CommentShape,
    pub doc: Option<CommentPlacement>,
}

/// Shape of a comment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommentShape {
    Line,
    Block,
}

/// Whether a doc comment is inner or outer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommentPlacement {
    /// `///` or `/** */`
    Outer,
    /// Sail doesn't have inner doc comments, but included for RA compat.
    Inner,
}

impl CommentKind {
    /// Classify a `SyntaxKind` into a `CommentKind`.
    pub fn from_syntax_kind(kind: SK) -> Option<CommentKind> {
        match kind {
            SK::LINE_COMMENT => Some(CommentKind { shape: CommentShape::Line, doc: None }),
            SK::BLOCK_COMMENT => Some(CommentKind { shape: CommentShape::Block, doc: None }),
            SK::DOC_COMMENT => {
                Some(CommentKind { shape: CommentShape::Line, doc: Some(CommentPlacement::Outer) })
            }
            _ => None,
        }
    }

    /// Whether this is a doc comment.
    pub fn is_doc(&self) -> bool {
        self.doc.is_some()
    }
}

/// Classification of string literal tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StringKind {
    /// `"..."` regular string
    Regular,
    /// `{|...|}`  multi-line string (Sail-specific)
    Multiline,
}
