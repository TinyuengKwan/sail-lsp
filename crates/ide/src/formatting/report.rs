//! Format report — error and warning collection.
//!

use ide_db::line_index::TextRange;

/// Formatting error kind.
#[derive(Debug, Clone)]
pub(crate) enum ErrorKind {
    /// Line exceeds max_width (actual, configured).
    LineOverflow(usize, usize),
    /// Trailing whitespace found.
    TrailingWhitespace,
    /// A comment was lost during formatting.
    LostComment,
}

/// Single formatting error with location.
#[derive(Debug, Clone)]
pub(crate) struct FormatError {
    pub(crate) kind: ErrorKind,
    pub(crate) range: TextRange,
}

/// Collects formatting errors and warnings.
#[derive(Debug, Default)]
pub(crate) struct FormatReport {
    pub(crate) errors: Vec<FormatError>,
}

impl FormatReport {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    pub(crate) fn push(&mut self, kind: ErrorKind, range: TextRange) {
        self.errors.push(FormatError { kind, range });
    }
}
