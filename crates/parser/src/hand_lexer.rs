//! Hand-written lexer for the Sail language.
//!
//! Produces the same output as the chumsky-based `lexer()` in `lexer.rs`:
//! a `Vec<(Token, Span)>` of non-trivia tokens with correct byte spans.
//! Whitespace and comments are skipped (they are handled by `lex_full.rs`).

use crate::lexer::{Span, Token};

/// Tokenize Sail source code, returning non-trivia tokens with byte spans.
///
/// Matches the output of the chumsky `lexer().parse(input)` exactly.
pub fn tokenize(input: &str) -> Vec<(Token, Span)> {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut pos = 0;
    let mut tokens = Vec::new();

    while pos < len {
        // Skip whitespace
        if bytes[pos].is_ascii_whitespace() {
            pos += 1;
            while pos < len && bytes[pos].is_ascii_whitespace() {
                pos += 1;
            }
            continue;
        }

        // Skip line comments: // ...
        if pos + 1 < len && bytes[pos] == b'/' && bytes[pos + 1] == b'/' {
            pos += 2;
            while pos < len && bytes[pos] != b'\n' {
                pos += 1;
            }
            continue;
        }

        // Skip block comments: /* ... */ (nested)
        if pos + 1 < len && bytes[pos] == b'/' && bytes[pos + 1] == b'*' {
            pos += 2;
            let mut depth = 1u32;
            while pos < len && depth > 0 {
                if pos + 1 < len && bytes[pos] == b'/' && bytes[pos + 1] == b'*' {
                    depth += 1;
                    pos += 2;
                } else if pos + 1 < len && bytes[pos] == b'*' && bytes[pos + 1] == b'/' {
                    depth -= 1;
                    pos += 2;
                } else {
                    pos += 1;
                }
            }
            continue;
        }

        // NOTE: Sail does NOT use OCaml-style (* *) comments.
        // Sail uses C-style /* */ and // comments (not OCaml-style).
        // The (* *) handler was incorrect — removed.

        let start = pos;

        // Multiline string: """..."""
        if pos + 2 < len && bytes[pos] == b'"' && bytes[pos + 1] == b'"' && bytes[pos + 2] == b'"' {
            pos += 3;
            let content_start = pos;
            loop {
                if pos + 2 < len
                    && bytes[pos] == b'"'
                    && bytes[pos + 1] == b'"'
                    && bytes[pos + 2] == b'"'
                {
                    let content = input[content_start..pos].to_owned();
                    pos += 3;
                    tokens.push((Token::MultilineString(content), Span::new(start, pos)));
                    break;
                }
                if pos >= len {
                    // Unterminated — emit what we have
                    let content = input[content_start..pos].to_owned();
                    tokens.push((Token::MultilineString(content), Span::new(start, pos)));
                    break;
                }
                pos += 1;
            }
            continue;
        }

        // Regular string: "..."
        if bytes[pos] == b'"' {
            pos += 1; // skip opening quote
            while pos < len && bytes[pos] != b'"' && bytes[pos] != b'\n' && bytes[pos] != b'\r' {
                if bytes[pos] == b'\\' {
                    pos += 1; // skip backslash
                    if pos < len {
                        match bytes[pos] {
                            b'x' => {
                                // \xHH
                                pos += 1;
                                // consume up to 2 hex digits
                                for _ in 0..2 {
                                    if pos < len && bytes[pos].is_ascii_hexdigit() {
                                        pos += 1;
                                    }
                                }
                            }
                            b'0'..=b'9' => {
                                // \DDD — exactly 3 decimal digits
                                pos += 1;
                                for _ in 0..2 {
                                    if pos < len && bytes[pos].is_ascii_digit() {
                                        pos += 1;
                                    }
                                }
                            }
                            _ => {
                                pos += 1; // \n, \t, \\, \", \', \r, \b, \<newline>
                            }
                        }
                    }
                } else {
                    pos += 1;
                }
            }
            if pos < len && bytes[pos] == b'"' {
                pos += 1; // skip closing quote
            }
            // Store the full slice including quotes (matches chumsky to_slice behavior)
            let s = input[start..pos].to_owned();
            tokens.push((Token::String(s), Span::new(start, pos)));
            continue;
        }

        // Type variable: 'ident
        if bytes[pos] == b'\'' {
            let after_quote = pos + 1;
            if after_quote < len && is_ident_start(bytes[after_quote]) {
                pos = after_quote + 1;
                while pos < len && is_ident_continue(bytes[pos]) {
                    pos += 1;
                }
                let s = input[start..pos].to_owned();
                tokens.push((Token::TyVal(s), Span::new(start, pos)));
                continue;
            }
            // lone quote — shouldn't normally occur, skip
            pos += 1;
            continue;
        }

        // Numbers: hex, bin, real, decimal
        if bytes[pos].is_ascii_digit() {
            // Hex: 0x...
            if bytes[pos] == b'0' && pos + 1 < len && bytes[pos + 1] == b'x' {
                pos += 2;
                while pos < len && (bytes[pos].is_ascii_hexdigit() || bytes[pos] == b'_') {
                    pos += 1;
                }
                let s = input[start..pos].to_owned();
                tokens.push((Token::Hex(s), Span::new(start, pos)));
                continue;
            }

            // Bin: 0b...
            if bytes[pos] == b'0' && pos + 1 < len && bytes[pos + 1] == b'b' {
                pos += 2;
                while pos < len && (bytes[pos] == b'0' || bytes[pos] == b'1' || bytes[pos] == b'_')
                {
                    pos += 1;
                }
                let s = input[start..pos].to_owned();
                tokens.push((Token::Bin(s), Span::new(start, pos)));
                continue;
            }

            // Decimal digits
            while pos < len && bytes[pos].is_ascii_digit() {
                pos += 1;
            }

            // Check for real: digits.digits
            if pos < len && bytes[pos] == b'.' && pos + 1 < len && bytes[pos + 1].is_ascii_digit() {
                pos += 1; // skip dot
                while pos < len && bytes[pos].is_ascii_digit() {
                    pos += 1;
                }
                let s = input[start..pos].to_owned();
                tokens.push((Token::Real(s), Span::new(start, pos)));
            } else {
                let s = input[start..pos].to_owned();
                tokens.push((Token::Num(s), Span::new(start, pos)));
            }
            continue;
        }

        // Directives: $ident...
        // Must come before single-char $ operator
        if bytes[pos] == b'$' && pos + 1 < len && is_ident_start_or_tilde(bytes[pos + 1]) {
            pos += 1; // skip $
            let name_start = pos;
            // Consume ident (including ~ as special)
            if bytes[pos] == b'~' {
                pos += 1;
            } else {
                pos += 1;
                while pos < len && is_ident_continue(bytes[pos]) {
                    pos += 1;
                }
            }
            let name = input[name_start..pos].to_owned();

            // Structured directive: $ident{
            if pos < len && bytes[pos] == b'{' {
                pos += 1; // consume the {
                tokens.push((Token::StructuredDirectiveStart(name), Span::new(start, pos)));
                continue;
            }

            // Regular directive: $ident rest_of_line
            let payload_start = pos;
            while pos < len && bytes[pos] != b'\n' {
                pos += 1;
            }
            let payload_text = &input[payload_start..pos];
            let payload =
                if payload_text.is_empty() { None } else { Some(payload_text.to_owned()) };
            tokens.push((Token::Directive { name, payload }, Span::new(start, pos)));
            continue;
        }

        // Special multi-char tokens that involve non-oper_char characters.
        // Must be checked before the generic oper_char scan.
        if pos + 1 < len {
            match [bytes[pos], bytes[pos + 1]] {
                [b'(', b')'] => {
                    pos += 2;
                    tokens.push((Token::Unit, Span::new(start, pos)));
                    continue;
                }
                [b'{', b'|'] => {
                    pos += 2;
                    tokens.push((Token::LeftCurlyBar, Span::new(start, pos)));
                    continue;
                }
                [b'[', b'|'] => {
                    pos += 2;
                    tokens.push((Token::LeftSquareBar, Span::new(start, pos)));
                    continue;
                }
                [b'|', b'}'] => {
                    pos += 2;
                    tokens.push((Token::RightCurlyBar, Span::new(start, pos)));
                    continue;
                }
                [b'|', b']'] => {
                    pos += 2;
                    tokens.push((Token::RightSquareBar, Span::new(start, pos)));
                    continue;
                }
                [b':', b':'] => {
                    pos += 2;
                    tokens.push((Token::Scope, Span::new(start, pos)));
                    continue;
                }
                [b':', b'='] => {
                    pos += 2;
                    tokens.push((Token::ColonEqual, Span::new(start, pos)));
                    continue;
                }
                [b'!', b'='] => {
                    pos += 2;
                    tokens.push((Token::NotEqualTo, Span::new(start, pos)));
                    continue;
                }
                _ => {}
            }
        }

        // Multi-char operators (longest match).
        // Handles all oper_char sequences including `_ident` suffixes.
        if let Some((tok, advance)) = try_multi_char_op(bytes, pos, len) {
            pos += advance;
            tokens.push((tok, Span::new(start, pos)));
            continue;
        }

        // Identifiers and keywords
        if is_ident_start_or_tilde(bytes[pos]) {
            if bytes[pos] == b'~' {
                // ~ alone is an identifier; it doesn't continue with more chars
                pos += 1;
                tokens.push((Token::Id("~".to_owned()), Span::new(start, pos)));
                continue;
            }

            pos += 1;
            while pos < len && is_ident_continue(bytes[pos]) {
                pos += 1;
            }
            let text = &input[start..pos];
            let tok = keyword_or_id(text);
            tokens.push((tok, Span::new(start, pos)));
            continue;
        }

        // Single-char operators (fallback)
        if let Some(tok) = single_char_op(bytes[pos]) {
            pos += 1;
            tokens.push((tok, Span::new(start, pos)));
            continue;
        }

        // Unknown character — skip (chumsky would error-recover)
        pos += 1;
    }

    tokens
}

#[inline]
fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'?'
}

#[inline]
fn is_ident_start_or_tilde(b: u8) -> bool {
    is_ident_start(b) || b == b'~'
}

#[inline]
fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'?' || b == b'\''
}

/// Try to match a multi-character operator at `pos`.
/// Returns `(Token, bytes_consumed)` or `None`.
/// Operator characters that form multi-char operators.
/// Excludes characters with dedicated grammar roles:
/// - `.` excluded: used as two separate DOT tokens for `x[31..0]` subranges.
/// - `:` excluded: used as COLON for type annotations `x : bits(32)`.
/// - `!` excluded: only valid in `!=` (handled as specific 2-char token).
fn is_oper_char(b: u8) -> bool {
    matches!(b, b'%' | b'&' | b'*' | b'+' | b'-' | b'/' | b'<' | b'=' | b'>' | b'@' | b'^' | b'|')
}

/// Try to lex a multi-character operator using longest-match.
///
/// Consumes the longest oper_char sequence. If it matches a known
/// structural token (like `<->`, `=>`, `==`, `<=`, etc.), we return that.
/// Otherwise, we return `InfixOp` for the whole sequence. This correctly
/// handles `<<<`, `>>>`, `>>`, `<<`, `==>`, and any user-defined
/// multi-char operators without hardcoding each one.
fn try_multi_char_op(bytes: &[u8], pos: usize, len: usize) -> Option<(Token, usize)> {
    let b0 = bytes[pos];
    if !is_oper_char(b0) {
        return None;
    }

    // Consume the longest oper_char sequence.
    let mut end = pos + 1;
    while end < len && is_oper_char(bytes[end]) {
        // Operator cannot start with comment
        // openings. The second char cannot be `*` if first is `/` (would
        // start `/*`), and cannot be `/` if first is `*` (would be `*/`
        // fragment). We apply a stricter rule: stop if we'd form `/*` or
        // `*/` at any position.
        if (bytes[end - 1] == b'/' && bytes[end] == b'*')
            || (bytes[end - 1] == b'*' && bytes[end] == b'/')
        {
            break;
        }
        end += 1;
    }

    let op_len = end - pos;
    let op_text = &bytes[pos..end];

    // Check for `_ident` suffix (upstream operatorn: `oper_char+ '_' ident`).
    // E.g., `<=_u`, `>=_si`, `<_s`.
    if end < len && bytes[end] == b'_' {
        let mut ext = end + 1;
        while ext < len && bytes[ext].is_ascii_alphanumeric() {
            ext += 1;
        }
        if ext > end + 1 {
            let full_op = std::str::from_utf8(&bytes[pos..ext]).unwrap_or("").to_owned();
            return Some((Token::InfixOp(full_op), ext - pos));
        }
    }

    // Single-char operators are handled by single_char_op in the caller.
    // Only return from here for multi-char (length >= 2).
    if op_len < 2 {
        return None;
    }

    // Match known structural tokens (longest first).
    // These need dedicated token variants for the parser grammar.
    match op_text {
        // 3-char
        b"<->" => return Some((Token::DoubleArrow, 3)),
        _ => {}
    }

    // For 2-char known tokens, only match if the full oper_char sequence
    // is exactly 2 chars (otherwise the sequence is a longer operator
    // like `>>>` or `==>` and should be InfixOp).
    // Known 2-char structural tokens (only oper_char combinations).
    // `::`, `:=`, `!=`, `{|`, `[|`, `|}`, `|]`, `()` are handled
    // in the pre-check above (they involve non-oper_char characters).
    if op_len == 2 {
        match op_text {
            b">=" => return Some((Token::GreaterThanOrEqualTo, 2)),
            b"=>" => return Some((Token::FatRightArrow, 2)),
            b"==" => return Some((Token::EqualTo, 2)),
            b"<=" => return Some((Token::LessThanOrEqualTo, 2)),
            b"<-" => return Some((Token::LeftArrow, 2)),
            b"->" => return Some((Token::RightArrow, 2)),
            _ => {}
        }
    }

    // Everything else is a generic operator (InfixOp).
    // This handles `>>`, `<<`, `<<<`, `>>>`, `==>`, `**`, and any
    // user-defined multi-char operator.
    let op_str = std::str::from_utf8(op_text).unwrap_or("").to_owned();
    Some((Token::InfixOp(op_str), op_len))
}

/// Match a single-character operator.
fn single_char_op(b: u8) -> Option<Token> {
    match b {
        b'$' => Some(Token::Dollar),
        b'#' => Some(Token::Hash),
        b'|' => Some(Token::Or),
        b'>' => Some(Token::GreaterThan),
        b'=' => Some(Token::Equal),
        b'<' => Some(Token::LessThan),
        b'+' => Some(Token::Plus),
        b'^' => Some(Token::Caret),
        b'%' => Some(Token::Modulus),
        b'&' => Some(Token::And),
        b'/' => Some(Token::Divide),
        b'*' => Some(Token::Multiply),
        b'@' => Some(Token::At),
        b'}' => Some(Token::RightCurlyBracket),
        b'{' => Some(Token::LeftCurlyBracket),
        b']' => Some(Token::RightSquareBracket),
        b'[' => Some(Token::LeftSquareBracket),
        b')' => Some(Token::RightBracket),
        b'(' => Some(Token::LeftBracket),
        b'.' => Some(Token::Dot),
        b':' => Some(Token::Colon),
        b';' => Some(Token::Semicolon),
        b',' => Some(Token::Comma),
        b'-' => Some(Token::Minus),
        b'_' => Some(Token::Underscore),
        b'!' => None, // ! alone is not a token (only != is)
        _ => None,
    }
}

/// Map an identifier string to a keyword token or `Token::Id`.
fn keyword_or_id(text: &str) -> Token {
    match text {
        "_" => Token::Underscore,
        "and" => Token::KwAnd,
        "as" => Token::KwAs,
        "assert" => Token::KwAssert,
        "backwards" => Token::KwBackwards,
        "barr" => Token::KwBarr,
        "bitfield" => Token::KwBitfield,
        "bitone" => Token::KwBitone,
        "bitzero" => Token::KwBitzero,
        "Bool" => Token::KwBool,
        "by" => Token::KwBy,
        "cast" => Token::KwCast,
        "catch" => Token::KwCatch,
        "case" => Token::KwCase,
        "clause" => Token::KwClause,
        "config" => Token::KwConfig,
        "configuration" => Token::KwConfiguration,
        "constant" => Token::KwConstant,
        "constraint" => Token::KwConstraint,
        "dec" => Token::KwDec,
        "default" => Token::KwDefault,
        "depend" => Token::KwDepend,
        "do" => Token::KwDo,
        "downto" => Token::KwDownto,
        "eamem" => Token::KwEamem,
        "effect" => Token::KwEffect,
        "else" => Token::KwElse,
        "end" => Token::KwEnd,
        "enum" => Token::KwEnum,
        "escape" => Token::KwEscape,
        "exit" => Token::KwExit,
        "exmem" => Token::KwExmem,
        "false" => Token::KwFalse,
        "forall" => Token::KwForall,
        "foreach" => Token::KwForeach,
        "forwards" => Token::KwForwards,
        "from" => Token::KwFrom,
        "function" => Token::KwFunction,
        "if" => Token::KwIf,
        "impl" => Token::KwImpl,
        "in" => Token::KwIn,
        "inc" => Token::KwInc,
        "infix" => Token::KwInfix,
        "infixl" => Token::KwInfixl,
        "infixr" => Token::KwInfixr,
        "instantiation" => Token::KwInstantiation,
        "Int" => Token::KwInt,
        "let" => Token::KwLet,
        "mapping" => Token::KwMapping,
        "match" => Token::KwMatch,
        "monadic" => Token::KwMonadic,
        "mutual" => Token::KwMutual,
        "mwv" => Token::KwMwv,
        "newtype" => Token::KwNewtype,
        "nondet" => Token::KwNondet,
        "Order" => Token::KwOrder,
        "outcome" => Token::KwOutcome,
        "overload" => Token::KwOverload,
        "private" => Token::KwPrivate,
        "pure" => Token::KwPure,
        "ref" => Token::KwRef,
        "register" => Token::KwRegister,
        "repeat" => Token::KwRepeat,
        "return" => Token::KwReturn,
        "rmem" => Token::KwRmem,
        "rreg" => Token::KwRreg,
        "scattered" => Token::KwScattered,
        "sizeof" => Token::KwSizeof,
        "struct" => Token::KwStruct,
        "switch" => Token::KwSwitch,
        "termination_measure" => Token::KwTerminationMeasure,
        "then" => Token::KwThen,
        "throw" => Token::KwThrow,
        "to" => Token::KwTo,
        "true" => Token::KwTrue,
        "try" => Token::KwTry,
        "type" => Token::KwType,
        "Type" => Token::KwTypeUpper,
        "undef" => Token::KwUndef,
        "undefined" => Token::KwUndefined,
        "union" => Token::KwUnion,
        "unspec" => Token::KwUnspec,
        "until" => Token::KwUntil,
        "val" => Token::KwVal,
        "var" => Token::KwVar,
        "when" => Token::KwWhen,
        "while" => Token::KwWhile,
        "with" => Token::KwWith,
        "wmem" => Token::KwWmem,
        "wreg" => Token::KwWreg,
        _ => Token::Id(text.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_val() {
        let tokens = tokenize("val");
        assert_eq!(tokens, vec![(Token::KwVal, Span::new(0, 3))]);
    }

    #[test]
    fn number_decimal() {
        let tokens = tokenize("42");
        assert_eq!(tokens, vec![(Token::Num("42".to_owned()), Span::new(0, 2))]);
    }

    #[test]
    fn number_hex() {
        let tokens = tokenize("0xFF");
        assert_eq!(tokens, vec![(Token::Hex("0xFF".to_owned()), Span::new(0, 4))]);
    }

    #[test]
    fn string_simple() {
        let tokens = tokenize("\"hello\"");
        assert_eq!(tokens, vec![(Token::String("\"hello\"".to_owned()), Span::new(0, 7))]);
    }

    #[test]
    fn operator_right_arrow() {
        let tokens = tokenize("->");
        assert_eq!(tokens, vec![(Token::RightArrow, Span::new(0, 2))]);
    }

    #[test]
    fn binary_literal() {
        let tokens = tokenize("0b1010_0101");
        assert_eq!(tokens, vec![(Token::Bin("0b1010_0101".to_owned()), Span::new(0, 11))]);
    }

    #[test]
    fn tyval_token() {
        let tokens = tokenize("'a");
        assert_eq!(tokens, vec![(Token::TyVal("'a".to_owned()), Span::new(0, 2))]);
    }

    #[test]
    fn real_number() {
        let tokens = tokenize("3.14");
        assert_eq!(tokens, vec![(Token::Real("3.14".to_owned()), Span::new(0, 4))]);
    }

    #[test]
    fn unit_token() {
        let tokens = tokenize("()");
        assert_eq!(tokens, vec![(Token::Unit, Span::new(0, 2))]);
    }

    #[test]
    fn tilde_identifier() {
        let tokens = tokenize("~");
        assert_eq!(tokens, vec![(Token::Id("~".to_owned()), Span::new(0, 1))]);
    }

    #[test]
    fn directive_with_payload() {
        let tokens = tokenize("$option --foo\nval x : int");
        assert_eq!(
            tokens[0],
            (
                Token::Directive { name: "option".to_owned(), payload: Some(" --foo".to_owned()) },
                Span::new(0, 13),
            )
        );
    }

    #[test]
    fn structured_directive() {
        let tokens = tokenize("$pragma{enabled = true}");
        assert_eq!(
            tokens[0],
            (Token::StructuredDirectiveStart("pragma".to_owned()), Span::new(0, 8),)
        );
    }

    #[test]
    fn multiline_string() {
        let tokens = tokenize("\"\"\"hello\"\"\"");
        assert_eq!(tokens, vec![(Token::MultilineString("hello".to_owned()), Span::new(0, 11))]);
    }

    /// J3-2: Multi-line content with actual newlines.
    #[test]
    fn multiline_string_with_newlines() {
        let input = "\"\"\"\n  line one\n  line two\n  \"\"\"";
        let tokens = tokenize(input);
        assert_eq!(tokens.len(), 1, "should produce single token");
        match &tokens[0].0 {
            Token::MultilineString(content) => {
                assert!(content.contains("line one"), "should contain line one");
                assert!(content.contains("line two"), "should contain line two");
            }
            other => panic!("expected MultilineString, got {:?}", other),
        }
    }

    #[test]
    fn skips_comments() {
        let tokens = tokenize("// comment\nval");
        assert_eq!(tokens, vec![(Token::KwVal, Span::new(11, 14))]);
    }

    #[test]
    fn skips_block_comments() {
        let tokens = tokenize("/* block */ val");
        assert_eq!(tokens, vec![(Token::KwVal, Span::new(12, 15))]);
    }

    #[test]
    fn no_ml_comments() {
        // Sail does NOT use OCaml-style (* *) comments.
        // (* ml *) should be lexed as separate tokens, not a comment.
        let tokens = tokenize("(* ml *) val");
        // ( * ml * ) val → 6 tokens
        assert!(tokens.len() >= 5, "expected (* ml *) to be tokens, not comment: {:?}", tokens);
        assert_eq!(tokens[0].0, Token::LeftBracket); // (
        assert_eq!(tokens[1].0, Token::Multiply); // *
    }

    #[test]
    fn double_arrow() {
        let tokens = tokenize("<->");
        assert_eq!(tokens, vec![(Token::DoubleArrow, Span::new(0, 3))]);
    }

    #[test]
    fn question_ident() {
        let tokens = tokenize("?foo");
        assert_eq!(tokens, vec![(Token::Id("?foo".to_owned()), Span::new(0, 4))]);
    }

    #[test]
    fn string_with_escape() {
        let tokens = tokenize(r#""he\nllo""#);
        assert_eq!(tokens, vec![(Token::String("\"he\\nllo\"".to_owned()), Span::new(0, 9))]);
    }

    #[test]
    fn underscore_as_keyword() {
        // Bare _ is Underscore (keyword), not Id
        let tokens = tokenize("_");
        assert_eq!(tokens, vec![(Token::Underscore, Span::new(0, 1))]);
    }

    // chumsky cross-check test removed (chumsky dependency deleted).
}
