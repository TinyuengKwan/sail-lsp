//! Text edit representation.
//!
//! sail-lsp simplified: single range + new_text (adequate for Sail).

use crate::line_index::TextRange;

/// A text edit: replace a range with new text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub range: TextRange,
    pub new_text: String,
}
