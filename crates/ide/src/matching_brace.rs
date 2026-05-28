//! Matching brace — find the counterpart of a bracket.
//! Supports: () [] {} {||} [||] pairs.

use ide_db::FileDb;
use parser::Token;

/// Bracket pairs for matching.
const PAIRS: &[(Token, Token)] = &[
    (Token::LeftBracket, Token::RightBracket),
    (Token::LeftSquareBracket, Token::RightSquareBracket),
    (Token::LeftCurlyBracket, Token::RightCurlyBracket),
    (Token::LeftCurlyBar, Token::RightCurlyBar),
    (Token::LeftSquareBar, Token::RightSquareBar),
];

/// Find the matching brace for the bracket at the given byte offset.
/// Returns the byte offset of the matching bracket, or None.
///
/// Direct token search, no delegation.
pub fn matching_brace(file: &dyn FileDb, offset: usize) -> Option<usize> {
    let tokens = file.tokens()?;

    // Find token at offset
    let idx = tokens.iter().position(|(_, span)| span.start <= offset && offset < span.end)?;
    let (token, _span) = &tokens[idx];

    // Find which pair this token belongs to
    for (open, close) in PAIRS {
        if token == open {
            // Scan forward for matching close
            let mut depth = 1i32;
            for (t, s) in tokens.iter().skip(idx + 1) {
                if t == open {
                    depth += 1;
                }
                if t == close {
                    depth -= 1;
                }
                if depth == 0 {
                    return Some(s.start);
                }
            }
            return None;
        }
        if token == close {
            // Scan backward for matching open
            let mut depth = 1i32;
            for i in (0..idx).rev() {
                let (t, s) = &tokens[i];
                if t == close {
                    depth += 1;
                }
                if t == open {
                    depth -= 1;
                }
                if depth == 0 {
                    return Some(s.start);
                }
            }
            return None;
        }
    }

    None
}
