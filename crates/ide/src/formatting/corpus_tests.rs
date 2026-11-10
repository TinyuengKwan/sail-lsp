#![allow(clippy::print_stderr)]

use super::*;

/// Exercise the formatter before its safety fallback, so returning the input
/// unchanged cannot hide a rewrite that would corrupt tokens or comments.
#[test]
#[ignore = "requires the sibling sail-riscv checkout; run explicitly for corpus validation"]
fn formatting_corpus_rewrites_preserve_tokens_and_settle() {
    fn collect(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "sail") {
                out.push(path);
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut paths = Vec::new();
    collect(&root.join("../sail-lsp/data/sail-lib"), &mut paths);
    collect(&root.join("../../../sail-riscv/model"), &mut paths);
    paths.sort();
    let output = std::env::var_os("SAIL_FORMAT_AUDIT_DIR").map(std::path::PathBuf::from);
    if let Some(dir) = &output {
        std::fs::create_dir_all(dir).unwrap();
    }
    let mut checked = 0;
    let mut changed = 0;
    let mut skipped = Vec::new();
    let mut failures = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        let source = std::fs::read_to_string(path).unwrap();
        let (_, errors) = syntax::parse_text(&source);
        if !errors.is_empty() {
            eprintln!("FORMAT_PARSE_SKIP {}: {errors:?}", path.display());
            skipped.push(path.display().to_string());
            continue;
        }
        for tab_size in [2, 4] {
            let options = FormatOptions { tab_size, ..Default::default() };
            checked += 1;
            let raw = raw_format(&source, &options);
            let tokens_ok = significant_tokens(&source) == significant_tokens(&raw);
            let parses = syntax::parse_text(&raw).1.is_empty();
            let second = raw_format(&raw, &options);
            let stable = second == raw;
            if raw != source {
                changed += 1;
            }
            if !tokens_ok || !parses || !stable {
                if let Some(dir) = &output {
                    std::fs::write(dir.join(format!("{index}-{tab_size}-before.sail")), &source)
                        .unwrap();
                    std::fs::write(dir.join(format!("{index}-{tab_size}-after.sail")), &raw)
                        .unwrap();
                    std::fs::write(dir.join(format!("{index}-{tab_size}-second.sail")), &second)
                        .unwrap();
                }
                failures.push(serde_json::json!({"index": index, "tab_size": tab_size, "path": path, "tokens_preserved": tokens_ok, "parses": parses, "idempotent": stable}));
            }
        }
    }
    let report = serde_json::json!({"checked": checked, "changed": changed, "parse_skipped": skipped, "failures": failures});
    if let Some(dir) = &output {
        std::fs::write(dir.join("report.json"), serde_json::to_string_pretty(&report).unwrap())
            .unwrap();
    }
    assert!(checked > 100, "unexpectedly small corpus: {report}");
    assert!(skipped.is_empty(), "formatter corpus contains untested files: {report}");
    assert!(failures.is_empty(), "formatter corpus defects: {report}");
}

fn raw_format(source: &str, options: &FormatOptions) -> String {
    let (tree, errors) = syntax::parse_text(source);
    assert!(errors.is_empty(), "{errors:?}");
    let snippet = snippet::SnippetProvider::new(source.to_owned());
    let mut visitor = visitor::FmtVisitor::new(options, &snippet);
    visitor.walk_source_file(&tree);
    let mut raw = normalize_blank_lines(&visitor.buffer);
    post_process_text(&mut raw, source, options);
    raw
}

#[test]
fn long_operator_chains_wrap_without_reassociating_operands() {
    let options = FormatOptions { tab_size: 2, max_line_width: Some(60), ..Default::default() };
    for source in [
        "function encode(x) = first_field(x) @ second_field(x) @ third_field(x) @ fourth_field(x)\n",
        "function display(x) = \"prefix @ ^\" ^ show_first(x) ^ show_second(x) ^ show_third(x)\n",
        "function encode(x) = (first_field(x) @ second_field(x)) @ third_field(x) @ fourth_field(x)\n",
        "function encode(x) = first_field(x) @ /* field boundary */ second_field(x) @ third_field(x)\n",
        "mapping m : string <-> string = { \"prefix\" ^ first_component(x) ^ second_component(x) ^ third_component(x) <-> x }\n",
    ] {
        let formatted = raw_format(source, &options);
        assert!(formatted.lines().any(|l| l.trim_start().starts_with(['@', '^'])), "not wrapped: {formatted}");
        assert_eq!(significant_tokens(source), significant_tokens(&formatted));
        assert!(syntax::parse_text(&formatted).1.is_empty(), "{formatted}");
        assert_eq!(raw_format(&formatted, &options), formatted);
        assert_eq!(format_document_cst(source, &options), formatted);
    }
}

#[test]
fn structural_rewrites_preserve_expression_bodies_comments_and_type_braces() {
    for source in [
        "function f(\"two  spaces\",\n x) = x\n",
        "function f(x : int,\n y : int) -> int = if x > y then x else y\n/// next function\nfunction g() = 0\n",
        "function f(\n x : int, // input\n y : int\n) -> int = x + y\n",
        "mapping m : {1, 2} <-> int = {\n 1 <-> 1,\n 2 <-> 2\n}\n",
        "struct S = {\n a : bits(if true\n then 8\n else 16),\n long_field : int\n}\n",
        "mapping m : bits(1) <-> string = {\n // comment\n 0b0 <-> \"zero\",\n (x : bits(1)) <-> \"one\"\n}\n",
    ] {
        let options = FormatOptions::default();
        let formatted = raw_format(source, &options);
        assert_eq!(significant_tokens(source), significant_tokens(&formatted), "{formatted}");
        assert_eq!(raw_format(&formatted, &options), formatted);
        assert_eq!(format_document_cst(source, &options), formatted);
    }
    let source = "struct S = {\n a : int,\n long_field : int\n}\n";
    assert_ne!(raw_format(source, &FormatOptions::default()), source);
}

#[test]
fn riscv_layout_prefers_body_breaks_and_preserves_tables() {
    let options = FormatOptions { tab_size: 2, ..Default::default() };
    let source = "function privLevel_to_virt_bit(p : Privilege) -> virt_mode_bit = { let (_, v) = privLevel_bits(p); v }\n";
    let expected = "function privLevel_to_virt_bit(p : Privilege) -> virt_mode_bit =\n  { let (_, v) = privLevel_bits(p); v }\n";
    assert_eq!(format_document_cst(source, &options), expected);
    assert_eq!(format_document_cst(expected, &options), expected);
    for source in [
        "function id forall ('a : Type) . (x : 'a) -> 'a = x\n",
        "function f(\n  x : int,\n  y : int\n) -> int = x + y\n",
        "mapping m : int <-> int = {\n  0    <-> 1,\n  1    <-> 2,\n\n  // another group\n  100  <-> 3\n}\n",
        "mapping clause encdec = FOO(rd)\n  <-> 0b00000000000000000000000000000000 @ encdec_reg(rd) @ 0b0110011\n  when enabled\n",
        "function f(x : int) -> int =\n  x +\n    x\n",
    ] {
        assert!(syntax::parse_text(source).1.is_empty(), "{source}");
        assert_eq!(format_document_cst(source, &options), source);
    }
    let source = "function f(x : int) -> int = { let y=x+1; y }\n";
    assert_eq!(
        format_document_cst(source, &options),
        "function f(x : int) -> int = { let y = x + 1; y }\n"
    );
}

#[test]
fn document_whitespace_does_not_rewrite_comment_or_string_contents() {
    let options = FormatOptions { tab_size: 2, ..Default::default() };
    let source = "\n\n/* keep\n\n\ncomment */\n\n\nfunction f() = {\n  let x = 1;\n\n\n  x\n}";
    let expected = "/* keep\n\n\ncomment */\n\nfunction f() = {\n  let x = 1;\n\n  x\n}\n";
    assert_eq!(format_document_cst(source, &options), expected);
    assert_eq!(format_document_cst(expected, &options), expected);
    let options = FormatOptions { insert_final_newline: Some(false), ..options };
    assert!(!format_document_cst("function f() = 1", &options).ends_with('\n'));
}
