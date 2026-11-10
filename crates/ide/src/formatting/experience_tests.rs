use super::*;
use ide_db::test_utils::TestFile;

fn apply(source: &str, edits: &[IdeTextEdit]) -> String {
    let mut result = source.to_owned();
    for edit in edits.iter().rev() {
        result.replace_range(
            usize::from(edit.range.start())..usize::from(edit.range.end()),
            &edit.new_text,
        );
    }
    result
}

#[test]
fn local_diffs_preserve_unicode_code_and_unchanged_definitions() {
    let source = "// 😀 unchanged\nfunction f() = { let x=1; x+2 }\nfunction same() = 7\n";
    let file = TestFile::new(source);
    let options = FormatOptions::default();
    let edits = format_document_edits(&file, &options).unwrap();
    assert!(edits.len() > 1);
    let tail = source.find("function same").unwrap();
    for edit in &edits {
        assert!(usize::from(edit.range.start()) > source.find("function f").unwrap());
        assert!(usize::from(edit.range.end()) < tail);
        assert!(source.is_char_boundary(usize::from(edit.range.start())));
    }
    let formatted = apply(source, &edits);
    assert_eq!(formatted, format_document_cst(source, &options));
    assert_eq!(format_document_cst(&formatted, &options), formatted);
    for (a, b) in [("α\r\n", "α\n"), ("😀x", "😀 x"), ("a", "a\n"), ("", "\n")] {
        assert_eq!(apply(a, &diff_edits(a, b)), b);
    }
}

#[test]
fn formatting_preserves_crlf_and_comment_and_string_contents() {
    let source = "// 😀 comment   \r\nfunction f() = { let s = \"two  spaces\"; let x=1; x }\r\n";
    let formatted = format_document_cst(source, &FormatOptions::default());
    assert!(formatted.contains("let x = 1"), "{formatted:?}");
    assert!(formatted.starts_with("// 😀 comment   \r\n"), "{formatted:?}");
    assert!(formatted.contains("\"two  spaces\""));
    assert!(!formatted.replace("\r\n", "").contains('\n'));
    assert_eq!(format_document_cst(&formatted, &FormatOptions::default()), formatted);
}

#[test]
fn block_documentation_keeps_markdown_line_breaks_and_blank_lines() {
    let comment = "/*!\n * first line  \n *\n *\n * second line\n */";
    let source = format!("{comment}\nfunction f() = {{ let x=1; x }}\n");
    let formatted = format_document_cst(&source, &FormatOptions::default());
    assert!(formatted.starts_with(comment), "{formatted:?}");
    assert!(formatted.contains("let x = 1"));
    assert_eq!(format_document_cst(&formatted, &FormatOptions::default()), formatted);
}

#[test]
fn selected_body_lines_format_without_touching_the_signature_or_neighbours() {
    let source = "function f(x:int) -> int = {\n  let y=x+1;\n  y+2\n}\nfunction same() = 7\n";
    let start = source.find("  let y").unwrap();
    let end = source.find("  y+2").unwrap();
    let file = TestFile::new(source);
    let range = base_db::text_range(start, end);
    let edits = range_format_document_edits(&file, range, &FormatOptions::default()).unwrap();
    assert!(edits.iter().all(|edit| range.contains_range(edit.range)));
    let after = apply(source, &edits);
    assert!(after.starts_with(&source[..start]));
    assert!(after.ends_with(&source[end..]));
    assert!(after.contains("let y = x + 1;"), "{after}");
    assert!(syntax::parse_text(&after).1.is_empty());
    assert_eq!(
        format_range(
            &after,
            &FormatOptions::default(),
            base_db::text_range(start, after.find("  y+2").unwrap())
        )
        .text,
        after
    );
}

#[test]
fn enter_obeys_spaces_tabs_and_never_replaces_code_before_the_cursor() {
    let source = "function f() = {\n    \n}\n";
    let file = TestFile::new(source);
    for (tab_size, insert_spaces, expected) in
        [(2, true, "  "), (4, true, "    "), (4, false, "\t")]
    {
        let options = FormatOptions { tab_size, insert_spaces, ..Default::default() };
        let edits =
            on_enter_edits_with_options(&file, ide_db::LineCol { line: 1, col: 4 }, &options)
                .unwrap();
        assert_eq!(edits[0].new_text, expected);
    }
    let file = TestFile::new("function f() = {\n  x\n}\n");
    assert!(on_enter_edits_with_options(
        &file,
        ide_db::LineCol { line: 1, col: 3 },
        &FormatOptions::default()
    )
    .is_none());
}

#[test]
fn enter_uses_lexical_context_while_code_is_incomplete() {
    let options = FormatOptions { tab_size: 4, ..Default::default() };
    let source = "function f() = { // open block\n\n}";
    let file = TestFile::new(source);
    assert_eq!(
        on_enter_edits_with_options(&file, ide_db::LineCol { line: 1, col: 0 }, &options).unwrap()
            [0]
        .new_text,
        "    "
    );
    let source = "/* comment\nthen\n\n*/\n";
    let file = TestFile::new(source);
    assert!(
        on_enter_edits_with_options(&file, ide_db::LineCol { line: 2, col: 0 }, &options).is_none()
    );
    let source = "function f() = {\n  let x = something_then\n\n}\n";
    let file = TestFile::new(source);
    assert_eq!(
        on_enter_edits_with_options(&file, ide_db::LineCol { line: 2, col: 0 }, &options).unwrap()
            [0]
        .new_text,
        "  "
    );
}

#[test]
fn typing_ignores_braces_and_arrows_in_strings_and_comments() {
    for source in [
        "function f() = {\n  let s = \"{\";\n  /* } { */\n      }\n",
        "function f() = {\n  // {\n      }\n",
    ] {
        let file = TestFile::new(source);
        let edits =
            crate::typing::on_char_typed(&file, source.rfind('}').unwrap() + 1, '}').unwrap();
        assert_eq!(edits[0].new_text, "");
    }
    for (source, needle, ch) in [
        ("// ->x\n", "->", '>'),
        ("/* ->x */\n", "->", '>'),
        ("let s = \"->x\"\n", "->", '>'),
        ("// }\n", "}", '}'),
    ] {
        let file = TestFile::new(source);
        assert!(crate::typing::on_char_typed(
            &file,
            source.find(needle).unwrap() + needle.len(),
            ch
        )
        .is_none());
    }
}

#[test]
fn explanations_distinguish_unchanged_invalid_and_range_exclusion() {
    let options = FormatOptions::default();
    assert_eq!(format_text("function f() = 1\n", &options).status, FormatStatus::AlreadyFormatted);
    let broken = format_text("function f(\n", &options);
    assert_eq!(broken.status, FormatStatus::ParseError);
    assert!(!broken.details.is_empty());
    let source = "function f() = { let x=1; x }\n";
    assert_eq!(
        format_range(source, &options, base_db::text_range(0, 1)).status,
        FormatStatus::RangeBoundary
    );
}

#[test]
fn overflow_reports_do_not_count_unicode_as_multiple_utf8_bytes() {
    let options = FormatOptions { tab_size: 2, max_line_width: Some(60), ..Default::default() };
    let source = format!("function f() = \"{}\"\n", "界".repeat(20));
    let result = format_text(&source, &options);
    assert!(!result.details.iter().any(|detail| detail.starts_with("Line width")), "{result:?}");
    assert_eq!(significant_tokens(&source), significant_tokens(&result.text));
}

#[test]
fn long_calls_branches_matches_and_conditions_have_stable_nested_layouts() {
    let options = FormatOptions { tab_size: 2, max_line_width: Some(50), ..Default::default() };
    for (source, marker) in [
        ("function f(x) = outer(inner(first_value(x), second_value(x)), third_value(x))\n", "outer(\n"),
        ("function f(x) = if x > 0 then first_long_function(x) else second_long_function(x)\n", "then\n"),
        ("function f(x) = match x { Some(y) => first_long_function(y), None => second_long_function(0) }\n", "{\n"),
        ("function f(x) = if first_condition(x) & second_condition(x) & third_condition(x) then 1 else 0\n", "\n    &"),
    ] {
        let result = format_text(source, &options);
        assert_eq!(result.status, FormatStatus::Changed, "{source}: {result:?}");
        assert!(result.text.contains(marker), "{marker}: {}", result.text);
        assert_eq!(significant_tokens(source), significant_tokens(&result.text));
        assert!(syntax::parse_text(&result.text).1.is_empty());
        assert_eq!(format_document_cst(&result.text, &options), result.text);
    }
}
