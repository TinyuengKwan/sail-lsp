//! Test helpers for assist handler tests.
//!
//! `check_assist` applies the assist and compares the file it produces, as
//! rust-analyzer's does. It used to return the assist's *labels*: 57 of the
//! 65 handlers' tests read only those, 33 passed the literal string
//! `"source code\n"` — which is not Sail — and none asserted on the text an
//! assist produces. That is how `add_explicit_type` shipped applying
//! nothing, and how `remove_parentheses` shipped unable to fire at all.

use crate::assist_context::{AssistContext, Assists, Handler};
use ide_db::test_utils::TestFile;

/// Apply the assist at `$0` and compare the resulting file.
///
/// `before` marks the cursor with `$0`. `after` is the whole file as the
/// assist leaves it.
pub(crate) fn check_assist(handler: Handler, before: &str, after: expect_test::Expect) {
    let (source, range) = extract_range(before);
    let applied = apply_first(handler, &source, range)
        .unwrap_or_else(|| panic!("the assist offered nothing at {range:?}"));
    after.assert_eq(&applied);
}

/// The same, for a client that can expand snippets: the assist's edit is
/// asserted to carry `$0` where the cursor should land, and the fixture
/// shows it in place.
pub(crate) fn check_assist_with_snippets(
    handler: Handler,
    before: &str,
    after: expect_test::Expect,
) {
    let (source, range) = extract_range(before);
    let cfg = crate::AssistConfig {
        snippet_cap: crate::SnippetCap::new(true),
        ..crate::AssistConfig::default()
    };
    let (applied, is_snippet) = apply_first_with(handler, &source, range, &cfg)
        .unwrap_or_else(|| panic!("the assist offered nothing at {range:?}"));
    assert!(is_snippet, "the change does not say it is a snippet");
    after.assert_eq(&applied);
}

/// The assist offers nothing here.
pub(crate) fn check_assist_not_applicable(handler: Handler, before: &str) {
    let (source, range) = extract_range(before);
    let labels = labels_in(handler, &source, range);
    assert!(labels.is_empty(), "expected no assist at {range:?}, got: {labels:?}");
}

/// The labels the assist offers, for the handlers that offer a choice.
pub(crate) fn assist_labels(handler: Handler, source: &str, cursor: usize) -> Vec<String> {
    labels_in(handler, source, base_db::text_range(cursor, cursor))
}

fn labels_in(handler: Handler, source: &str, range: base_db::TextRange) -> Vec<String> {
    let file = TestFile::new(source);
    let cfg = crate::AssistConfig::default();
    let ctx = AssistContext::new(&cfg, &file, range);
    let mut acc = Assists::new(base_db::FileId::from_raw(0));
    handler(&mut acc, &ctx);
    acc.finish().iter().map(|a| a.label.to_string()).collect()
}

/// The file after applying the handler's first assist, if it offers one.
fn apply_first(handler: Handler, source: &str, range: base_db::TextRange) -> Option<String> {
    let cfg = crate::AssistConfig::default();
    apply_first_with(handler, source, range, &cfg).map(|(text, _)| text)
}

fn apply_first_with(
    handler: Handler,
    source: &str,
    range: base_db::TextRange,
    cfg: &crate::AssistConfig,
) -> Option<(String, bool)> {
    let file = TestFile::new(source);
    let ctx = AssistContext::new(cfg, &file, range);
    let mut acc = Assists::new(base_db::FileId::from_raw(0));
    handler(&mut acc, &ctx);
    let assist = acc.finish().into_iter().next()?;
    let change = assist.source_change?;
    let is_snippet = change.is_snippet;
    let mut edits: Vec<_> = change.source_file_edits.into_values().flatten().collect();
    // Applied back to front, so an earlier edit does not move a later one.
    edits.sort_by_key(|e| std::cmp::Reverse(base_db::range_start(e.range)));
    let mut out = source.to_owned();
    for edit in edits {
        out.replace_range(
            base_db::range_start(edit.range)..base_db::range_end(edit.range),
            &edit.new_text,
        );
    }
    Some((out, is_snippet))
}

/// Split the `$0` markers out of a fixture.
///
/// One marker is a cursor; two are the ends of a selection. Several assists
/// read the selected text rather than a point — with only a cursor to offer
/// them, they see an empty range and decline, which is what the tests that
/// passed `("source code\n", 0)` were quietly recording.
fn extract_range(fixture: &str) -> (String, base_db::TextRange) {
    let first = fixture.find("$0").expect("the fixture has no `$0`");
    let rest = fixture.replacen("$0", "", 1);
    match rest[first..].find("$0") {
        Some(offset) => {
            let second = first + offset;
            (rest.replacen("$0", "", 1), base_db::text_range(first, second))
        }
        None => (rest, base_db::text_range(first, first)),
    }
}

/// Every assist that is offered can be applied.
///
/// `Assist` carried two fields for one thing: `source_change`, which is
/// rust-analyzer's, and `edits`, "a Sail bridge ... until handlers adopt
/// SourceChangeBuilder". Fifty-seven handlers filled the bridge, one filled
/// `source_change`, and the IDE layer read only the bridge — so
/// `add_explicit_type` reached the lightbulb menu with nothing to apply.
#[test]
fn every_offered_assist_has_something_to_apply() {
    use ide_db::test_utils::TestFile;

    // One source per shape an assist looks for, so the sweep below actually
    // offers something.
    const SOURCES: &[&str] = &[
        "val f : unit -> unit\nfunction f() = { let x = 1; () }\n",
        "scattered function execute\nfunction clause execute(x) = x\n",
        "struct S = { a : int, b : bool }\nfunction get(s) = s.a\n",
        "enum Color = Red | Green | Blue\nfunction f(c) = match c { Red => 1 }\n",
        "function add(x, y) = x + y\nfunction main() = add(1, 2)\n",
        "bitfield B : bits(8) = { hi : 7 .. 4, lo : 3 .. 0 }\nregister r : B\n",
        "union U = { X : int, Y : (int, bool) }\nfunction f(u) = match u { X(n) => n, Y(a, b) => a }\n",
        "mapping m : bits(2) <-> string = { 0b00 <-> \"a\", 0b01 <-> \"b\" }\n",
        "function f(x : int) -> int = if x > 0 then { var y = x; y } else 0\n",
        "type T = bits(8)\nval g : forall 'n, 'n > 0. int('n) -> unit\nfunction g(n) = ()\n",
        "function f() -> unit = { let (a, b) = (1, 2); assert(a > 0); () }\n",
    ];

    let cfg = crate::AssistConfig::default();
    let mut offered = 0usize;
    // What an assist produces has to be Sail. `check_assist` compares exact
    // text per handler, which only covers the handlers someone wrote a
    // fixture for; this is the machine-checkable generalisation, and it
    // found two handlers whose edits do not parse. Eighteen handlers still
    // build code with `format!`, so this is the ratchet that keeps the rest
    // honest.
    let syntax_errors = |t: &str| syntax::parse_text(t).1.len();
    let mut unparseable: std::collections::BTreeSet<String> = Default::default();
    for src in SOURCES {
        let before = syntax_errors(src);
        let file = TestFile::new(src);
        for at in 0..src.len() {
            for assist in crate::assists(&cfg, &file, base_db::text_range(at, at)) {
                offered += 1;
                let change = assist
                    .source_change
                    .as_ref()
                    .unwrap_or_else(|| panic!("`{}` is offered with no change", assist.label.0));
                assert!(!change.is_empty(), "`{}` is offered with an empty change", assist.label.0);

                let mut edits: Vec<_> =
                    change.source_file_edits.values().flatten().cloned().collect();
                // Back to front, so an earlier edit does not move a later one.
                edits.sort_by_key(|e| std::cmp::Reverse(base_db::range_start(e.range)));
                let mut out = src.to_string();
                let mut applied = true;
                for e in edits {
                    let (s, t) = (base_db::range_start(e.range), base_db::range_end(e.range));
                    if s > t
                        || t > out.len()
                        || !out.is_char_boundary(s)
                        || !out.is_char_boundary(t)
                    {
                        applied = false;
                        break;
                    }
                    out.replace_range(s..t, &e.new_text);
                }
                // `$0` is a snippet cursor, not text.
                let out = out.replace("$0", "");
                if !applied || syntax_errors(&out) > before {
                    unparseable.insert(assist.label.0.to_string());
                }
            }
        }
    }
    assert!(offered > 0, "the sweep offered no assists, so it checked nothing");
    assert!(
        unparseable.is_empty(),
        "these assists produce source that does not parse: {unparseable:?}"
    );
}

#[cfg(test)]
mod tree_edits {
    use ide_db::test_utils::TestFile;

    /// The parenthesis assist edits the tree, and the edit is minimal.
    #[test]
    fn remove_parentheses_replaces_only_the_pair() {
        let src = "val f : int -> int\nfunction f(x) = (x + 1)\n";
        let at = src.find("(x + 1)").expect("the parens");
        let file = TestFile::new(src);
        let cfg = crate::AssistConfig::default();
        let assist = crate::assists(&cfg, &file, base_db::text_range(at, at))
            .into_iter()
            .find(|a| a.id.0 == "remove_parentheses")
            .expect("the assist is offered");
        let change = assist.source_change.expect("it has a change");
        let edits: Vec<_> = change.source_file_edits.values().flatten().collect();
        assert_eq!(edits.len(), 1, "one replacement: {edits:?}");
        let edit = edits[0];
        let mut after = src.to_string();
        let start = base_db::range_start(edit.range);
        let end = base_db::range_end(edit.range);
        after.replace_range(start..end, &edit.new_text);
        assert_eq!(after, "val f : int -> int\nfunction f(x) = x + 1\n");
    }
}

/// No assist test asserts on a program that is not Sail.
///
/// Thirty-three of them passed the literal `"source code\n"` and read the
/// assist's label, under a comment saying "verify no panic". That is a test
/// that a handler does not crash on text it cannot parse, and it was the
/// whole coverage for a third of the handlers.
#[test]
fn assist_tests_run_on_sail() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/handlers");
    for entry in std::fs::read_dir(dir).expect("the handlers directory") {
        let path = entry.expect("a directory entry").path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("a handler source");
        assert!(
            !text.contains("assist_labels(") || !text.contains("\"source code"),
            "{}: a test asserts on text that is not Sail",
            path.display()
        );
    }
}

/// A handler finds code in the tree, not in the file's bytes.
///
/// Every handler that recovered structure from raw text got it wrong in
/// the same way: the word it searched for was found inside a comment or a
/// string literal, or the delimiter it counted was. `merge_nested_if`
/// merged an `if` written in a string; `generate_new` split a field list
/// on `,` and read `vector(2, dec, bit)` as three fields; `flip_comma`
/// walked back to the nearest `(` and swapped across a nested call;
/// `convert_comment_block` called every line between two block comments
/// part of one.
///
/// The remaining uses are formatting, not structure: indentation off the
/// current line, and rendering a comment's own text.
#[test]
fn a_handler_finds_code_in_the_tree() {
    const STRUCTURE_BY_TEXT: &[&str] = &[
        "find_matching_brace",
        "find_matching_paren",
        "find_keyword_near",
        "find_keyword_at",
        "find_function_keyword",
    ];
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/handlers");
    let mut offenders = Vec::new();
    for entry in std::fs::read_dir(dir).expect("the handlers directory") {
        let path = entry.expect("a directory entry").path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("a handler source");
        for needle in STRUCTURE_BY_TEXT {
            if text.contains(needle) {
                offenders.push(format!(
                    "{}: {needle}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ));
            }
        }
    }
    offenders.sort();
    assert_eq!(offenders, Vec::<String>::new(), "structure recovered from raw text");
}

/// An assist that generates something to fill in says where to fill it.
///
/// Six do. The generate-* handlers that are *not* here — `generate_new`,
/// `generate_getter_or_setter`, `add_scattered_end` — produce complete
/// code with nothing to type over (`function mk_P(x : int) -> P = struct
/// { x = x }`, `end f`), and rust-analyzer places a tabstop only where
/// there is something to fill. A `$0` there would be noise.
#[test]
fn an_assist_with_a_hole_says_where_it_is() {
    const WITH_TABSTOPS: &[(&str, &str)] = &[
        ("generate_function", include_str!("handlers/generate_function.rs")),
        ("generate_val_spec", include_str!("handlers/generate_val_spec.rs")),
        (
            "generate_documentation_template",
            include_str!("handlers/generate_documentation_template.rs"),
        ),
        ("generate_mapping_clause", include_str!("handlers/generate_mapping_clause.rs")),
        ("add_missing_match_arms", include_str!("handlers/add_missing_match_arms.rs")),
        ("add_missing_fields", include_str!("handlers/add_missing_fields.rs")),
    ];
    for (name, src) in WITH_TABSTOPS {
        assert!(
            src.contains("ctx.config.snippet_cap"),
            "{name} generates code without saying where to type"
        );
    }
}
