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
    let options = FormatOptions::default();
    let mut checked = 0;
    let mut changed = 0;
    let mut skipped = Vec::new();
    let mut failures = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        let source = std::fs::read_to_string(path).unwrap();
        let (_, errors) = syntax::parse_text(&source);
        if !errors.is_empty() {
            skipped.push(path.display().to_string());
            continue;
        }
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
                std::fs::write(dir.join(format!("{index}-before.sail")), &source).unwrap();
                std::fs::write(dir.join(format!("{index}-after.sail")), &raw).unwrap();
                std::fs::write(dir.join(format!("{index}-second.sail")), &second).unwrap();
            }
            failures.push(serde_json::json!({"index": index, "path": path, "tokens_preserved": tokens_ok, "parses": parses, "idempotent": stable}));
        }
    }
    let report = serde_json::json!({"checked": checked, "changed": changed, "parse_skipped": skipped, "failures": failures});
    if let Some(dir) = &output {
        std::fs::write(dir.join("report.json"), serde_json::to_string_pretty(&report).unwrap())
            .unwrap();
    }
    assert!(checked > 100, "unexpectedly small corpus: {report}");
    assert!(failures.is_empty(), "formatter corpus defects: {report}");
}

fn raw_format(source: &str, options: &FormatOptions) -> String {
    let (tree, errors) = syntax::parse_text(source);
    assert!(errors.is_empty(), "{errors:?}");
    let snippet = snippet::SnippetProvider::new(source.to_owned());
    let mut visitor = visitor::FmtVisitor::new(options, &snippet);
    visitor.walk_source_file(&tree);
    format_lines(&mut visitor.buffer, options, &mut visitor.report);
    let mut raw = visitor.buffer;
    post_process_text(&mut raw, source, options);
    raw
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
