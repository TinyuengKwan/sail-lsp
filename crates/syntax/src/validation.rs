//! Tree-level checks run after parsing, folded into a `Parse`'s errors.
//!
//! Mirrors rust-analyzer's `syntax::validation`: the parser reports what it
//! could not consume, validation reports what it consumed but should not
//! have. Keeping both in one place means every consumer — the diagnostics
//! pipeline, `view_syntax_tree`, tests — sees the same set instead of each
//! having to remember an extra pass.

use crate::syntax_error::SyntaxError;
use crate::syntax_node;
use crate::syntax_node::SyntaxNode;
use parser::SyntaxKind as SK;
use rowan::TextRange;

/// Validate a parsed syntax tree, appending to `errors`.
pub(crate) fn validate(root: &SyntaxNode, errors: &mut Vec<SyntaxError>) {
    validate_brackets(root, errors);
    validate_conditionals(root, errors);
    validate_option_pragmas(root, errors);
    validate_if_has_then(root, errors);
    validate_doc_comments(root, errors);
    validate_visibility_modifiers(root, errors);
}

/// `if c then e` — the `then` is not optional.
///
/// Upstream has only `If_ exp Then exp` and `If_ exp Then exp Else exp`
/// (`parser.mly:663-673`); `if c { 1 }` is a *syntax* error there, reported
/// at the token that should have been `then`:
///
/// ```text
/// function t() -> int = { if c { 1 } }
///                              ^  current token: {
/// ```
///
/// The parser stays lenient about it on purpose — rust-analyzer's rule, and
/// the reason `validation` exists: a half-typed `if` should still produce a
/// tree the rest of the IDE can work on. Reporting belongs here, and this
/// layer had only two checks in it (bracket balance, `$ifdef` balance) while
/// rust-analyzer's has a dozen.
fn validate_if_has_then(root: &SyntaxNode, errors: &mut Vec<SyntaxError>) {
    for node in root.descendants().filter(|n| n.kind() == SK::IF_EXPR) {
        let mut children = node.children_with_tokens();
        // `if`
        children.next();
        // The condition, then whatever follows it.
        let mut seen_cond = false;
        let mut at: Option<rowan::TextSize> = None;
        let mut has_then = false;
        for el in children {
            if el.kind() == SK::KW_THEN {
                has_then = true;
                break;
            }
            if el.kind().is_trivia() {
                continue;
            }
            if seen_cond && at.is_none() {
                at = Some(el.text_range().start());
            }
            seen_cond = true;
        }
        if !has_then {
            // Upstream draws its caret on the token that should have been
            // `then` (`current token: {`). Without a `then` the parser takes
            // no branch, so the `if` node ends at the condition and that
            // token is not inside it — the span here is the incomplete `if`
            // itself, which is one token to the left of upstream's.
            let _ = at;
            errors.push(SyntaxError::new("expected `then`".to_string(), node.text_range()));
        }
    }
}

/// One item, one documentation comment.
///
/// `Ann_doc (doc, x, l)` over something that already has one raises
/// (`initial_check.ml:1809` for a field, with siblings at `:1110` for a
/// union constructor, `:2015`/`:2107` for a function clause and `:2238`
/// for a toplevel definition — the wording is per item kind and so is
/// this).
///
/// What makes two comments two rather than one is upstream's lexer rule
/// `"\n" wsc* "///"` (`lexer.mll:311`): consecutive `///` lines are *one*
/// `DocLine` token, so a paragraph break written as `///` on its own line
/// stays one comment, and a genuinely blank line starts a second. An
/// attribute between them does not separate them — `Ann_attribute` wraps
/// and recurses (`:1806`), which is exactly the case upstream's own
/// `test/lexing/line_doc_comment.sail` is built around.
fn validate_doc_comments(root: &SyntaxNode, errors: &mut Vec<SyntaxError>) {
    // Start of each doc *block* attached to whatever comes next.
    let mut run: Vec<syntax_node::SyntaxToken> = Vec::new();
    let mut block_open = false;
    let mut attr_depth = 0i32;
    let mut after_dollar = false;

    for el in root.descendants_with_tokens() {
        let Some(token) = el.into_token() else { continue };
        let kind = token.kind();

        if attr_depth > 0 {
            match kind {
                SK::L_BRACK => attr_depth += 1,
                SK::R_BRACK => attr_depth -= 1,
                _ => {}
            }
            continue;
        }
        if after_dollar {
            after_dollar = false;
            if kind == SK::L_BRACK {
                attr_depth = 1;
                block_open = false;
                continue;
            }
        }

        match kind {
            SK::WHITESPACE => {
                if token.text().matches('\n').count() >= 2 {
                    // A blank line ends the block. The run continues: both
                    // comments still annotate the next item.
                    block_open = false;
                }
            }
            SK::DOC_COMMENT | SK::BLOCK_COMMENT
                if kind == SK::DOC_COMMENT || token.text().starts_with("/*!") =>
            {
                let is_block = token.text().starts_with("/*!");
                if !block_open || is_block {
                    run.push(token.clone());
                    if run.len() == 2 {
                        errors.push(SyntaxError::new_at_offset(
                            multiple_docs_message(&token),
                            token.text_range().start(),
                        ));
                    }
                }
                block_open = !is_block;
            }
            SK::LINE_COMMENT | SK::BLOCK_COMMENT => block_open = false,
            SK::DOLLAR => {
                after_dollar = true;
                block_open = false;
            }
            _ => {
                // An item. Whatever follows starts its own run.
                run.clear();
                block_open = false;
            }
        }
    }
}

/// Upstream words this per item kind, so this reads the kind rather than
/// picking one.
fn multiple_docs_message(token: &syntax_node::SyntaxToken) -> String {
    for node in token.parent_ancestors() {
        match node.kind() {
            // A field list — `struct`, `bitfield` and `union` share the
            // node, and only the introducing keyword tells them apart.
            SK::STRUCT_EXPR => {
                let is_union = node
                    .parent()
                    .into_iter()
                    .flat_map(|p| p.children_with_tokens())
                    .any(|el| el.kind() == SK::KW_UNION);
                return if is_union {
                    "Union constructor has multiple documentation comments".to_string()
                } else {
                    "Field has multiple documentation comments".to_string()
                };
            }
            SK::SCATTERED_CLAUSE_DEF | SK::CALLABLE_DEF => {
                return "Function clause has multiple documentation comments".to_string()
            }
            _ => {}
        }
    }
    // At the top level the comment is a sibling of what it annotates, not
    // a descendant of it, so the ancestors say nothing — look forward.
    let next = token
        .siblings_with_tokens(rowan::Direction::Next)
        .skip(1)
        .find(|el| !el.kind().is_trivia() && el.kind() != SK::DOC_COMMENT);
    if next.map(|el| el.kind()) == Some(SK::SCATTERED_CLAUSE_DEF) {
        return "Function clause has multiple documentation comments".to_string();
    }
    "Toplevel definition has multiple documentation comments".to_string()
}

/// Modifiers wrap a declaration; repeating visibility does not create a
/// second declaration. Keep recovery permissive and report at the duplicate.
fn validate_visibility_modifiers(root: &SyntaxNode, errors: &mut Vec<SyntaxError>) {
    let mut private = false;
    for token in root.descendants_with_tokens().filter_map(|el| el.into_token()) {
        if token.kind().is_trivia() || token.kind() == SK::DOC_COMMENT {
            continue;
        }
        if token.parent_ancestors().any(|n| n.kind() == SK::ATTRIBUTE) {
            continue;
        }
        if token.kind() == SK::KW_PRIVATE {
            if private {
                errors.push(SyntaxError::new(
                    "Definition has multiple visibility modifiers",
                    token.text_range(),
                ));
            }
            private = true;
        } else {
            private = false;
        }
    }
}

/// The bracket pairs Sail writes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BracketKind {
    Round,
    Square,
    Curly,
    CurlyBar,
    SquareBar,
}

impl BracketKind {
    fn closer(self) -> &'static str {
        match self {
            BracketKind::Round => ")",
            BracketKind::Square => "]",
            BracketKind::Curly => "}",
            BracketKind::CurlyBar => "|}",
            BracketKind::SquareBar => "|]",
        }
    }

    fn opening(kind: SK) -> Option<BracketKind> {
        Some(match kind {
            SK::L_PAREN => BracketKind::Round,
            SK::L_BRACK => BracketKind::Square,
            SK::L_CURLY => BracketKind::Curly,
            SK::L_CURLY_BAR => BracketKind::CurlyBar,
            SK::L_BRACKET_BAR => BracketKind::SquareBar,
            // The lexer folds a structured pragma's opening brace into the
            // introducer, so `$c_override {` is one token and never pushes a
            // brace of its own.
            SK::STRUCTURED_DIRECTIVE_START => BracketKind::Curly,
            _ => return None,
        })
    }

    fn closing(kind: SK) -> Option<BracketKind> {
        Some(match kind {
            SK::R_PAREN => BracketKind::Round,
            SK::R_BRACK => BracketKind::Square,
            SK::R_CURLY => BracketKind::Curly,
            SK::R_CURLY_BAR => BracketKind::CurlyBar,
            SK::R_BRACKET_BAR => BracketKind::SquareBar,
            _ => return None,
        })
    }
}

/// Every bracket is closed, by its own closer, in order.
///
/// The parser recovers from an unbalanced bracket instead of reporting it, so
/// without this a stray `}` shows up as a cascade of "expected definition"
/// further down the file rather than as the thing that is actually wrong.
fn validate_brackets(root: &SyntaxNode, errors: &mut Vec<SyntaxError>) {
    let mut stack: Vec<BracketKind> = Vec::new();
    let mut last_end = root.text_range().start();

    for token in root.descendants_with_tokens().filter_map(|el| el.into_token()) {
        let range: TextRange = token.text_range();
        last_end = range.end();
        if let Some(kind) = BracketKind::opening(token.kind()) {
            stack.push(kind);
            continue;
        }
        let Some(close) = BracketKind::closing(token.kind()) else { continue };

        let mut matched = false;
        while let Some(open) = stack.last().copied() {
            if open == close {
                stack.pop();
                matched = true;
                break;
            }
            // A closer belonging to an enclosing pair arrived first, so
            // everything above it on the stack was never closed.
            errors.push(SyntaxError::new_at_offset(
                format!("expected '{}'", open.closer()),
                range.start(),
            ));
            stack.pop();
        }
        if !matched {
            errors.push(SyntaxError::new(format!("unexpected '{}'", close.closer()), range));
        }
    }

    // Only the innermost few: one missing brace should not become a wall of
    // identical errors at EOF.
    for open in stack.into_iter().rev().take(5) {
        errors.push(SyntaxError::new_at_offset(format!("expected '{}'", open.closer()), last_end));
    }
}

/// `$ifdef` / `$ifndef` / `$iftarget` are closed by `$endif`.
///
/// `raise (err_general (pragma_loc l) "$ifdef, $ifndef, or $iftarget never
/// ended by $endif")` (`preprocess.ml:126`) — upstream's wording, and its
/// caret is on the *opening* directive, which is the one the reader has to
/// go and fix. This said "1 unclosed $ifdef/$ifndef directive(s)" at the
/// end of the file: a count where upstream names a place.
///
/// Two checks that used to live here are gone. Neither had ever run — the
/// salsa parse query bypassed `Parse::errors()` — and running them showed
/// both were wrong: a repeated `$include` of the same file is ordinary Sail
/// (include guards make it idempotent, `smt/option_tuple_i683.unsat.sail`
/// does it deliberately), and reporting every top-level `ERROR` node just
/// restated what the parser had already said about the same text.
fn validate_conditionals(root: &SyntaxNode, errors: &mut Vec<SyntaxError>) {
    let mut open: Vec<rowan::TextSize> = Vec::new();

    for child in root.children() {
        if child.kind() != SK::DIRECTIVE_DEF {
            continue;
        }
        let text = child.text().to_string();
        let trimmed = text.trim();
        let offset = child.text_range().start();

        if trimmed.starts_with("$ifdef")
            || trimmed.starts_with("$ifndef")
            || trimmed.starts_with("$iftarget")
        {
            open.push(offset);
        } else if trimmed.starts_with("$endif") && open.pop().is_none() {
            errors.push(SyntaxError::new_at_offset(
                "$endif without matching $ifdef".to_string(),
                offset,
            ));
        }
    }

    for offset in open {
        errors.push(SyntaxError::new_at_offset(
            "$ifdef, $ifndef, or $iftarget never ended by $endif".to_string(),
            offset,
        ));
    }
}

/// `$option` takes the flags `sail` itself takes, and two things it cannot.
///
/// `Arg.parse_argv` over the directive's words, with `file_arg` raising on
/// anything that is not a flag (`preprocess.ml:210-216`). Two of its three
/// outcomes need no knowledge of `sail`'s flag table:
///
/// - a word that does not start with `-` reaches `file_arg`, which is the
///   anonymous-argument error — `$option file.sail` is someone reaching
///   for `$include`;
/// - `-help` raises `Arg.Help`, which upstream turns into an error rather
///   than printing usage into the middle of a type check.
///
/// The third — `Arg.Bad`, an unknown flag — is a *warning* upstream and
/// needs the flag table to decide. It is not checked here: the table is
/// this compiler's command line, it moves between releases, and a stale
/// copy would report a flag that works. Named in `NOT_REPORTED`
/// (`parser::lex_full`) with the same reason.
fn validate_option_pragmas(root: &SyntaxNode, errors: &mut Vec<SyntaxError>) {
    for child in root.children() {
        if child.kind() != SK::DIRECTIVE_DEF {
            continue;
        }
        let text = child.text().to_string();
        let Some(args) = text.trim().strip_prefix("$option ") else { continue };
        let offset = child.text_range().start();
        let mut words = args.split_whitespace();
        // `Arg.parse_argv` hands a flag's own value to the flag, not to
        // `file_arg`, so which *later* words are in argument position needs
        // the table. The first word does not: nothing precedes it to
        // consume it.
        if let Some(first) = words.next() {
            if !first.starts_with('-') {
                errors.push(SyntaxError::new_at_offset(
                    format!("Anonymous argument '{first}' cannot be passed via $option directive"),
                    offset,
                ));
                continue;
            }
        }
        if args.split_whitespace().any(|w| w == "-help" || w == "--help") {
            errors.push(SyntaxError::new_at_offset(
                "-help flag passed to $option directive".to_string(),
                offset,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::validate;
    use crate::parsing::parse_text;

    fn messages(text: &str) -> Vec<String> {
        let (root, _) = parse_text(text);
        let mut errors = Vec::new();
        validate(&root, &mut errors);
        errors.iter().map(|e| e.message().to_string()).collect()
    }

    #[test]
    fn balanced_source_is_clean() {
        assert!(messages("function f() = { let x = [1, 2]; x }\n").is_empty());
    }

    #[test]
    fn an_unmatched_closer_is_named() {
        assert_eq!(messages("function f() = x }\n"), ["unexpected '}'"]);
    }

    #[test]
    fn an_unclosed_opener_is_reported_at_eof() {
        assert_eq!(messages("function f() = {\n"), ["expected '}'"]);
    }

    #[test]
    fn a_structured_pragma_carries_its_brace() {
        assert!(messages("$c_override {\n  id = a\n}\n").is_empty());
        assert_eq!(messages("$c_override {\n  id = a\n"), ["expected '}'"]);
    }

    #[test]
    fn an_unclosed_conditional_is_reported() {
        assert_eq!(
            messages("$ifdef FOO\nval f : int -> int\n"),
            ["$ifdef, $ifndef, or $iftarget never ended by $endif"]
        );
    }

    #[test]
    fn a_repeated_include_is_not_an_error() {
        // Include guards make it idempotent; upstream accepts it.
        assert!(messages("$include <option.sail>\n$include <option.sail>\n").is_empty());
    }
}
