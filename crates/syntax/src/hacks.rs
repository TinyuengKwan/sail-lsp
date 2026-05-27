//! Compatibility hacks.
//! Things which exist to work around various fun Sail parser/syntax
//! edge cases. Ideally empty.

use crate::SyntaxToken;

/// Check if a token is a multi-line string that might affect parsing.
///
/// Sail has `"..."` and multi-line string literals. This can be used
/// to detect edge cases in incremental reparsing.
pub fn token_is_multiline_string(token: &SyntaxToken) -> bool {
    use parser::SyntaxKind;
    token.kind() == SyntaxKind::MULTILINE_STRING_LIT
}
