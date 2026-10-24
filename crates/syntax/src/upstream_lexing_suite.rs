//! Upstream's own lexer test suite, case by case.
//!
//! `sail/test/lexing/` is eighteen files, each beside a `.expect` holding
//! the exact output `sail` produces — message and caret. It is the only
//! upstream corpus that ships an expected *position*, which makes it the
//! right oracle for the layers whose whole job is to say where something
//! is.
//!
//! The cases are transcribed here rather than read from the checkout,
//! because they are a few lines each and a test that silently skips when a
//! sibling repo is missing holds nothing.
//!
//! It asks `SourceFile::parse(..).errors()` rather than any one producer,
//! for the reason `corpus_check` states in its own header: measure through
//! the entry point the product uses. Upstream raises these from three
//! places — the lexer, the preprocessor and `initial_check` — and a reader
//! in an editor sees one list.

use crate::SourceFile;

/// `line`/`col` are 1-based line and 0-based column, exactly as upstream's
/// `.expect` prints them (`3.6-6` is line 3, column 6).
struct Case {
    name: &'static str,
    source: &'static str,
    line: usize,
    col: usize,
    message: &'static str,
}

const CASES: &[Case] = &[
    Case {
        name: "import_reserved",
        source: "default Order dec\n\nimport",
        line: 3,
        col: 0,
        message: "import is a reserved keyword",
    },
    Case {
        name: "module_reserved",
        source: "default Order dec\n\nmodule",
        line: 3,
        col: 0,
        message: "module is a reserved keyword",
    },
    Case {
        name: "extended_ascii",
        source: "default Order dec\n\nval main : unit -> unit\n\n\
                 function main() -> unit = {\n  let _ = \u{2122};\n  ()\n}\n",
        line: 6,
        col: 10,
        message: "Unexpected character: \\226",
    },
    Case {
        name: "illegal_escape",
        source: "default Order dec\n\nfunction main() -> unit = {\n  let x = \"\\f\"\n}\n",
        line: 4,
        col: 11,
        message: "String literal contains illegal backslash escape sequence",
    },
    Case {
        name: "unterminated_string",
        source: "default Order dec\n\nfunction main() -> unit = {\n  let x = \"\n}\n",
        line: 4,
        col: 10,
        message: "String literal not terminated",
    },
    Case {
        name: "unbalanced_comment",
        source: "default Order dec\n\n/* */ */\n",
        line: 3,
        col: 6,
        message: "Unbalanced comment",
    },
    // `/* /* */` closes the inner comment and leaves the outer open. It is
    // here because the two readers of a block comment disagreed about
    // that: `hand_lexer` counted depth and `emit_trivia` did not.
    Case {
        name: "unbalanced_comment2",
        source: "default Order dec\n\n/* /* */\n\nval main : unit -> unit\n",
        line: 3,
        col: 0,
        message: "Unbalanced comment",
    },
    Case {
        name: "doc_comment_eof",
        source: "default Order dec\n\n/*! doc\ncomment\n",
        line: 3,
        col: 0,
        message: "Unbalanced documentation comment",
    },
    Case {
        name: "line_comment_eof",
        source: "default Order dec\n\n// comment",
        line: 3,
        col: 0,
        message: "File ended before newline in comment",
    },
    Case {
        name: "pragma_block",
        source: "default Order dec\n\n$include <prelude.sail> /* block comment */ more\n",
        line: 3,
        col: 44,
        message: "Directive continued after block comment",
    },
    Case {
        name: "mls_dedent",
        source: "\nlet foo : string = \"\"\"\n    some text\n    x\n    y\n    z\n      abc\n\
                 \x20 dedent\n    more text\n    \"\"\"\n",
        line: 8,
        col: 0,
        message: "Line 6 in multi-line string literal has an invalid prefix.\n\
                  It must contain the exact sequence of whitespace characters that preceed \
                  the final \"\"\" delimiter.",
    },
    Case {
        name: "unterminated_ifdef",
        source: "\n$ifdef\n",
        line: 2,
        col: 0,
        message: "$ifdef, $ifndef, or $iftarget never ended by $endif",
    },
    Case {
        name: "bad_option_pragma",
        source: "\n$option file.sail\n",
        line: 2,
        col: 0,
        message: "Anonymous argument 'file.sail' cannot be passed via $option directive",
    },
    Case {
        name: "line_doc_comment",
        source: "default Order dec\n\nbitfield quux : bits(32) = {\n  /// Another doc comment\n\
                 \x20 $[attr]\n  /// Second doc comment!\n  some_field : 31 .. 16\n}\n",
        line: 6,
        col: 2,
        message: "Field has multiple documentation comments",
    },
    Case {
        name: "help_option_pragma",
        source: "\n$option -help\n",
        line: 2,
        col: 0,
        message: "-help flag passed to $option directive",
    },
];

/// Cases from the same directory that nothing here reports.
///
/// The list may only shrink. Each is upstream rejecting a file this
/// server takes, which is a false negative — the cheap direction, and the
/// reason they are recorded rather than guessed at:
///
/// - `cr_invisible` — a bare `\r` matches no rule upstream (`ws` is space
///   and tab, `lexer.mll:172`), so a file saved with Windows line endings
///   is rejected outright. Reporting it means an error on *every line* of
///   such a file, and the decision taken was not to: a user who opens a
///   CRLF file wants to be told once, by something that can offer to fix
///   it, not by the lexer sixty times.
/// - `bad_option_pragma2` — `Arg.Bad`, an unknown flag, is a *warning*
///   upstream (`preprocess.ml:218`) and needs `sail`'s own flag table to
///   decide. That table is this compiler's command line; it moves between
///   releases, and a stale copy would report a flag that works. The rest
///   of that file — its unclosed `$ifdef` — is reported.
/// - `unknown_operator` — reported, but by `ide-diagnostics` rather than
///   here: deciding it needs `WorkspaceFixities`, which this layer has no
///   way to ask. Held by `an_undeclared_operator_is_reported` there
///   instead.
const NOT_REPORTED: &[&str] = &["cr_invisible", "bad_option_pragma2", "unknown_operator"];

fn offset_of(source: &str, line: usize, col: usize) -> usize {
    let start: usize = source.split_inclusive('\n').take(line - 1).map(str::len).sum();
    start + col
}

/// Every case reports upstream's message at upstream's caret.
#[test]
fn an_upstream_error_lands_where_upstream_draws_its_caret() {
    for case in CASES {
        let parse = SourceFile::parse(case.source);
        let want = offset_of(case.source, case.line, case.col);
        let got: Vec<_> = parse
            .errors()
            .iter()
            .map(|e| (usize::from(e.range().start()), e.to_string()))
            .collect();
        assert!(
            got.iter().any(|(off, msg)| *off == want && msg == case.message),
            "{}: expected {:?} at {want} ({}.{}), got {got:#?}",
            case.name,
            case.message,
            case.line,
            case.col
        );
    }
}

/// A file upstream accepts draws none of them. Upstream's suite is all
/// rejections, so nothing in it holds this direction.
#[test]
fn a_file_upstream_accepts_draws_no_error() {
    let source = "default Order dec\n\
                  $include <prelude.sail> /* fine, the directive ends here */\n\
                  /* a /* nested */ comment */\n\
                  val f : int -> int\n\
                  function f(x) = x\n\
                  let s : string = \"a\\tb\\x41\\065\\\\\"\n\
                  let m : string = \"\"\"\n  indented\n\n  lines\n  \"\"\"\n";
    let errors = SourceFile::parse(source).errors();
    assert!(errors.is_empty(), "clean file: {errors:#?}");
}

/// The baseline is a list, not a comment: it has to name cases that exist.
#[test]
fn the_baseline_names_upstreams_own_cases() {
    assert_eq!(
        CASES.len() + NOT_REPORTED.len(),
        18,
        "sail/test/lexing has 18 cases; this list must account for all of them"
    );
}

#[test]
fn where_is_reserved_since_sail_0a0e2a017() {
    let source = "function f() = { let where = 1; where }\n";
    let errors = SourceFile::parse(source).errors();
    assert!(errors
        .iter()
        .any(|e| usize::from(e.range().start()) == 21
            && e.to_string() == "where is a reserved keyword"));
    assert!(SourceFile::parse("let somewhere = \"where\" /* where */\n").errors().is_empty());
}
