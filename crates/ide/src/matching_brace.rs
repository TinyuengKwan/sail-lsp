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

#[cfg(test)]
mod tests {
    use crate::fixture;

    /// Through `Analysis`, which is the door the server now has.
    ///
    /// Two implementations of this existed — this one and
    /// `formatting::matching_brace_offset`, the same token walk written
    /// twice — and neither had a caller, so no editor could ask.
    #[test]
    fn a_brace_finds_its_counterpart() {
        let source = "function f() = { g(1) }\n";
        let (analysis, files) = fixture::analysis(source);
        let url = analysis.url_for_file_id(files[0].0).expect("fixture file has a url");

        let open = source.find('{').expect("fixture");
        let close = source.find('}').expect("fixture");
        assert_eq!(analysis.matching_brace(url, open).unwrap(), Some(close));
        assert_eq!(analysis.matching_brace(url, close).unwrap(), Some(open));

        // The inner pair, not the outer one. Note `f()`'s brackets are not a
        // pair to look for: the lexer gives `()` as one `UNIT` token, because
        // in Sail it is the unit value.
        let inner_open = source.find("g(").expect("fixture") + 1;
        let inner_close = source.rfind(')').expect("fixture");
        assert_eq!(analysis.matching_brace(url, inner_open).unwrap(), Some(inner_close));

        // Not a bracket — the client leaves that cursor where it is.
        let name = source.find('f').expect("fixture");
        assert_eq!(analysis.matching_brace(url, name).unwrap(), None);
    }
}
