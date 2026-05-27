//! Syntax errors produced during parsing.

use rowan::TextRange;

/// A syntax error with a message and text range.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SyntaxError(String, TextRange);

impl SyntaxError {
    /// Create a syntax error spanning a range.
    pub fn new(message: impl Into<String>, range: TextRange) -> Self {
        Self(message.into(), range)
    }

    /// Create a syntax error at a single offset.
    pub fn new_at_offset(message: impl Into<String>, offset: rowan::TextSize) -> Self {
        Self(message.into(), TextRange::empty(offset))
    }

    /// The text range of this error.
    pub fn range(&self) -> TextRange {
        self.1
    }

    /// The error message.
    pub fn message(&self) -> &str {
        &self.0
    }

    /// Return a copy of this error with a different range.
    pub fn with_range(self, range: TextRange) -> SyntaxError {
        SyntaxError(self.0, range)
    }
}

impl std::fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for SyntaxError {}

#[cfg(test)]
mod tests {
    use super::*;
    use rowan::TextSize;

    #[test]
    fn new_with_range() {
        let range = TextRange::new(TextSize::from(10), TextSize::from(20));
        let err = SyntaxError::new("expected `;`", range);
        assert_eq!(err.message(), "expected `;`");
        assert_eq!(err.range(), range);
    }

    #[test]
    fn new_at_offset() {
        let err = SyntaxError::new_at_offset("unexpected token", TextSize::from(42));
        assert_eq!(err.message(), "unexpected token");
        assert_eq!(err.range(), TextRange::empty(TextSize::from(42)));
    }

    #[test]
    fn display_shows_message() {
        let err = SyntaxError::new_at_offset("bad syntax", TextSize::from(0));
        assert_eq!(format!("{err}"), "bad syntax");
    }

    #[test]
    fn equality() {
        let range = TextRange::new(TextSize::from(0), TextSize::from(5));
        let a = SyntaxError::new("err", range);
        let b = SyntaxError::new("err", range);
        assert_eq!(a, b);
    }
}
