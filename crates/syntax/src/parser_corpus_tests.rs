//! Snapshot tests for the parser, over a corpus of `.sail` files.
//!
//! rust-analyzer's shape (`parser/test_data/parser/{ok,err}`, compared with
//! `expect_file!`): each case is a source file beside a `.rast` holding the
//! whole expected tree. Borrowed because the assertions this repo had been
//! writing by hand are strictly weaker, and one of them demonstrably missed
//! a defect.
//!
//! `foreach (i from 0 to 32 by 1 in inc) { .. }` parsed with a *correct*
//! span — the definition covered the whole source — while the loop's body
//! was the `by` step and the block was a sibling. A span assertion passes on
//! that; the tree does not. It was found by dumping the HIR instead, which
//! is one layer too late.
//!
//! Regenerate after an intentional grammar change:
//!
//! ```sh
//! UPDATE_EXPECT=1 cargo test -p syntax parser_corpus
//! ```

use expect_test::expect_file;

fn corpus(dir: &str, want_errors: bool) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test_data/parser").join(dir);
    let mut cases: Vec<_> = std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("{}: {e}", root.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "sail"))
        .collect();
    cases.sort();
    assert!(!cases.is_empty(), "no cases in {}", root.display());

    for case in cases {
        let text = std::fs::read_to_string(&case).expect("case is readable");
        let parse = crate::SourceFile::parse(&text);
        let name = case.file_stem().unwrap_or_default().to_string_lossy().to_string();
        assert_eq!(
            !parse.errors().is_empty(),
            want_errors,
            "{name}: expected {} errors",
            if want_errors { "some" } else { "no" }
        );
        expect_file![case.with_extension("rast")].assert_eq(&parse.debug_dump());
    }
}

/// Every form here parses cleanly, and its tree is pinned.
#[test]
fn parser_corpus_ok() {
    corpus("ok", false);
}

/// Forms upstream rejects. Pinned too: how a parse *fails* decides how much
/// of the file survives, and this repo's own P0 list starts with a parse
/// failure that loses a whole function.
#[test]
fn parser_corpus_err() {
    corpus("err", true);
}

/// Every token span can slice the text it came from.
///
/// Both lexers had a "skip the character I do not know" branch that
/// advanced one *byte*, so a span could end inside a character and
/// `LexedStr::text` panicked slicing at it. Upstream's own
/// `test/lexing/extended_ascii.sail` — `let _ = \u{2122};` — crashed the
/// parser, where `sail` reports "Unexpected character: \226" and carries
/// on.
///
/// Not a snapshot: the property is that no offset the lexer produces can
/// be un-sliceable, which holds for text no case file contains.
#[test]
fn every_token_span_is_on_a_character_boundary() {
    for text in [
        "function t() = { let _ = \u{2122}; }\n",
        "// Union\u{2019}s Horizon 2020\nval x : int\n",
        "val \u{4f60}\u{597d} : int\n",
        "function t() = { \"\u{1f600}\" }\n",
        "\u{2122}\u{2122}\u{2122}\n",
    ] {
        let parse = crate::SourceFile::parse(text);
        for el in parse.syntax_node().descendants_with_tokens() {
            let r = el.text_range();
            let (s, e) = (usize::from(r.start()), usize::from(r.end()));
            assert!(
                text.is_char_boundary(s) && text.is_char_boundary(e),
                "{text:?}: span {s}..{e} cannot slice the text"
            );
        }
        // And the errors it reports are placeable too.
        for err in parse.errors() {
            let r = err.range();
            let (s, e) = (usize::from(r.start()), usize::from(r.end()));
            assert!(
                text.is_char_boundary(s) && text.is_char_boundary(e),
                "{text:?}: error span {s}..{e} cannot slice the text"
            );
        }
    }
}

/// A field list carries no node that stands for no text.
///
/// `$[attr]` before a field is ordinary Sail — upstream's own
/// `test/lexing/line_doc_comment.sail` puts one in a bitfield — and the
/// three near-copies of the field-list loop all fell through to their
/// safety valve on it, one token at a time, leaving an empty `FIELD_INIT`
/// beside each. The third copy had no valve at all: it could only
/// terminate on the parser's global step limit, which is the budget the
/// rest of the file needs.
///
/// The property, rather than the fix: a node with an empty range says a
/// consumer found something where there is nothing.
#[test]
fn a_field_list_has_no_empty_nodes() {
    for src in [
        "struct S = {\n  $[attr]\n  x : int\n}\n",
        "bitfield B : bits(4) = {\n  $[attr]\n  x : 3 .. 0\n}\n",
        "union U = {\n  $[attr]\n  C : int\n}\n",
        "let s = struct { $[attr] x = 1 }\n",
        // Nested brackets in the payload: the attribute ends at the `]`
        // that matches, not the first one.
        "struct S = {\n  $[f { xs = [\"a\", \"b\"] }]\n  x : int\n}\n",
    ] {
        let parse = crate::SourceFile::parse(src);
        let root = parse.syntax_node();
        let empty: Vec<_> = root
            .descendants()
            .filter(|n| n.text_range().is_empty())
            .map(|n| (n.kind(), n.text_range()))
            .collect();
        assert!(empty.is_empty(), "{src:?} produced empty nodes: {empty:?}");
        // And the definition still covers the whole source.
        assert_eq!(
            root.children().next().map(|d| d.text_range().end()),
            Some(rowan::TextSize::from(src.trim_end().len() as u32)),
            "{src:?}: the definition does not cover its own text"
        );
    }
}
