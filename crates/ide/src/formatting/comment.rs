//! Comment style identification and handling.
//!

/// Comment style classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommentStyle {
    /// `//` line comment
    DoubleSlash,
    /// `///` doc comment
    TripleSlash,
    /// `//!` inner doc comment
    DocBang,
    /// `/* ... */` block comment
    SingleBlock,
    /// `/** ... */` doc block comment
    DocBlock,
}

impl CommentStyle {
    /// Detect comment style from text.
    pub(crate) fn from_str(s: &str) -> Option<Self> {
        let trimmed = s.trim_start();
        if trimmed.starts_with("///") {
            Some(CommentStyle::TripleSlash)
        } else if trimmed.starts_with("//!") {
            Some(CommentStyle::DocBang)
        } else if trimmed.starts_with("//") {
            Some(CommentStyle::DoubleSlash)
        } else if trimmed.starts_with("/**") {
            Some(CommentStyle::DocBlock)
        } else if trimmed.starts_with("/*") {
            Some(CommentStyle::SingleBlock)
        } else {
            None
        }
    }

    /// Is this a line comment style?
    pub(crate) fn is_line_comment(&self) -> bool {
        matches!(
            self,
            CommentStyle::DoubleSlash | CommentStyle::TripleSlash | CommentStyle::DocBang
        )
    }

    /// Is this a block comment style?
    pub(crate) fn is_block_comment(&self) -> bool {
        matches!(self, CommentStyle::SingleBlock | CommentStyle::DocBlock)
    }

    /// Is this a documentation comment?
    pub(crate) fn is_doc_comment(&self) -> bool {
        matches!(self, CommentStyle::TripleSlash | CommentStyle::DocBang | CommentStyle::DocBlock)
    }

    /// The opening characters for this comment style.
    pub(crate) fn opener(&self) -> &'static str {
        match self {
            CommentStyle::DoubleSlash => "// ",
            CommentStyle::TripleSlash => "/// ",
            CommentStyle::DocBang => "//! ",
            CommentStyle::SingleBlock => "/* ",
            CommentStyle::DocBlock => "/** ",
        }
    }

    /// The closing characters for this comment style.
    pub(crate) fn closer(&self) -> &'static str {
        match self {
            CommentStyle::SingleBlock | CommentStyle::DocBlock => " */",
            _ => "",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_double_slash() {
        assert_eq!(CommentStyle::from_str("// hello"), Some(CommentStyle::DoubleSlash));
    }

    #[test]
    fn detect_triple_slash() {
        assert_eq!(CommentStyle::from_str("/// doc"), Some(CommentStyle::TripleSlash));
    }

    #[test]
    fn detect_block() {
        assert_eq!(CommentStyle::from_str("/* block */"), Some(CommentStyle::SingleBlock));
    }

    #[test]
    fn detect_doc_block() {
        assert_eq!(CommentStyle::from_str("/** doc block */"), Some(CommentStyle::DocBlock));
    }

    #[test]
    fn non_comment() {
        assert_eq!(CommentStyle::from_str("let x = 1"), None);
    }
}
