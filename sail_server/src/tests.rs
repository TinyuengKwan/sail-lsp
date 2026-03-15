use super::Parameter;
use super::*;
use tower_lsp::lsp_types::Diagnostic;

#[test]
fn finds_call_and_argument_index() {
    let source = r#"
function add(x, y) = x + y
function main() = add(1, 2)
"#;
    let file = File::new(source.to_string());
    let call_offset = source.find("2)").unwrap();
    let pos = file.source.position_at(call_offset);
    let call = find_call_at_position(&file, pos);
    assert_eq!(call, Some(("add".to_string(), 1)));
}

#[test]
fn finds_call_and_argument_index_in_top_level_initializer() {
    let source = r#"
val add : (int, int) -> int
function add(x, y) = x + y
let result = add(1, 2)
"#;
    let file = File::new(source.to_string());
    let call_offset = source.find("2)").unwrap();
    let pos = file.source.position_at(call_offset);
    let call = find_call_at_position(&file, pos);
    assert_eq!(call, Some(("add".to_string(), 1)));
}

#[test]
fn infers_call_argument_types_in_mapping_clause_via_expr_parser_fallback() {
    let source = r#"
val use_bits : bits(8) -> int
mapping clause assembly = use_bits(0x12) <-> "ok"
"#;
    let file = File::new(source.to_string());
    let uri = Url::parse("file:///tmp/main.sail").unwrap();
    let call_offset = source.find("use_bits(").unwrap() + 2;
    let pos = file.source.position_at(call_offset);
    let files = vec![(&uri, &file)];

    let arg_types =
        hover::support::infer_call_arg_types_at_position(&files, &uri, &file, pos, "use_bits")
            .expect("arg types");
    assert_eq!(arg_types, vec![Some("bits(8)".to_string())]);
}

#[test]
fn collects_callable_signatures() {
    let source = r#"
val add : (int, int) -> int
function add(x, y) = x + y
"#;
    let file = File::new(source.to_string());
    let signatures = collect_callable_signatures(&file);
    assert!(signatures.iter().any(|sig| sig.name == "add"));
}

#[test]
fn builds_function_snippet() {
    let params = vec![
        Parameter {
            name: "x".to_string(),
            is_implicit: false,
        },
        Parameter {
            name: "y : int".to_string(),
            is_implicit: false,
        },
    ];
    assert_eq!(function_snippet("add", &params), "add(${1:x}, ${2:y})");
}

#[test]
fn completion_triggers_do_not_include_whitespace() {
    let triggers = completion_trigger_characters();
    assert!(!triggers.iter().any(|t| t.trim().is_empty()));
    assert!(triggers.contains(&".".to_string()));
    assert!(triggers.contains(&":".to_string()));
}

#[test]
fn infers_binding_type_for_literals() {
    assert_eq!(
        infer_binding_type(&sail_parser::Token::Num("1".into())),
        Some("int")
    );
    assert_eq!(
        infer_binding_type(&sail_parser::Token::String("x".into())),
        Some("string")
    );
    assert_eq!(
        infer_binding_type(&sail_parser::Token::KwTrue),
        Some("bool")
    );
}

#[test]
fn offers_missing_semicolon_fix() {
    let source = "function f() = {\n  let x = 1\n}\n";
    let file = File::new(source.to_string());
    let diagnostic = Diagnostic::new_simple(
        Range::new(
            tower_lsp::lsp_types::Position::new(1, 2),
            tower_lsp::lsp_types::Position::new(1, 10),
        ),
        "expected ';'".to_string(),
    );

    let edit = missing_semicolon_fix(&file, &diagnostic).expect("expected quick fix");
    assert_eq!(edit.new_text, ";");
}

#[test]
fn offers_missing_closer_fix() {
    let source = "function f() = {\n  let x = (1 + 2\n}\n";
    let file = File::new(source.to_string());
    let diagnostic = Diagnostic::new_simple(
        Range::new(
            tower_lsp::lsp_types::Position::new(1, 16),
            tower_lsp::lsp_types::Position::new(1, 16),
        ),
        "expected ')'".to_string(),
    );

    let (_, edit, _) = quick_fix_for_diagnostic(&file, &diagnostic).expect("expected fix");
    assert_eq!(edit.new_text, ")");
}

#[test]
fn captures_return_type_from_val_signature() {
    let source = "val f : int -> bits(32)\nfunction f(x) = x\n";
    let file = File::new(source.to_string());
    let signatures = collect_callable_signatures(&file);
    let f = signatures
        .into_iter()
        .find(|sig| sig.name == "f")
        .expect("missing signature");
    assert_eq!(f.return_type.as_deref(), Some("bits(32)"));
}

#[test]
fn offers_missing_comma_fix() {
    let source = "function f() = [1 2]\n";
    let file = File::new(source.to_string());
    let diagnostic = Diagnostic::new_simple(
        Range::new(
            tower_lsp::lsp_types::Position::new(0, 17),
            tower_lsp::lsp_types::Position::new(0, 17),
        ),
        "expected ','".to_string(),
    );

    let (_, edit, _) = quick_fix_for_diagnostic(&file, &diagnostic).expect("expected fix");
    assert_eq!(edit.new_text, ",");
}

#[test]
fn offers_missing_equal_fix() {
    let source = "let x 1\n";
    let file = File::new(source.to_string());
    let diagnostic = Diagnostic::new_simple(
        Range::new(
            tower_lsp::lsp_types::Position::new(0, 6),
            tower_lsp::lsp_types::Position::new(0, 6),
        ),
        "expected '='".to_string(),
    );

    let (_, edit, _) = quick_fix_for_diagnostic(&file, &diagnostic).expect("expected fix");
    assert_eq!(edit.new_text, "=");
}

#[test]
fn builds_selection_range_chain() {
    let source = "function f() = {\n  let x = (1 + 2);\n}\n";
    let file = File::new(source.to_string());
    let pos = tower_lsp::lsp_types::Position::new(1, 13);
    let selection = make_selection_range(&file, pos);
    assert!(range_len(&file, &selection.range) > 0);
    assert!(selection.parent.is_some());
}

#[test]
fn builds_call_edges_for_file() {
    let source = r#"
function foo(x) = bar(x)
function bar(x) = x
"#;
    let file = File::new(source.to_string());
    let uri = Url::parse("file:///tmp/test.sail").unwrap();
    let edges = call_edges_for_file(&uri, &file);
    assert!(edges.iter().any(|e| e.caller == "foo" && e.callee == "bar"));
}

#[test]
fn parses_named_type() {
    assert_eq!(parse_named_type("bits(32)"), None);
    assert_eq!(parse_named_type("my_struct"), Some("my_struct".to_string()));
    assert_eq!(
        parse_named_type("option(my_type)"),
        Some("option".to_string())
    );
}

#[test]
fn extracts_typed_bindings() {
    let file = File::new("let x : my_type = 1".to_string());
    let bindings = typed_bindings(&file);
    assert_eq!(bindings.get("x"), Some(&"my_type".to_string()));
}

#[test]
fn extracts_typed_function_parameter_bindings() {
    let file = File::new("function f(x : bits(32), y : int) = x".to_string());
    let bindings = typed_bindings(&file);
    assert_eq!(bindings.get("x"), Some(&"bits(32)".to_string()));
    assert_eq!(bindings.get("y"), Some(&"int".to_string()));
}

#[test]
fn extracts_typed_var_bindings() {
    let file = File::new("function f() = { var x : bits(32) = 0x0; x }".to_string());
    let bindings = typed_bindings(&file);
    assert_eq!(bindings.get("x"), Some(&"bits(32)".to_string()));
}

#[test]
fn does_not_treat_types_as_function_parameter_names() {
    let source = "function f(x : bits(32), y : int) -> bits(32) = x\n";
    let file = File::new(source.to_string());
    let sig = collect_callable_signatures(&file)
        .into_iter()
        .find(|sig| sig.name == "f")
        .expect("missing signature");

    let params = sig
        .params
        .into_iter()
        .map(|param| param.name)
        .collect::<Vec<_>>();
    assert_eq!(
        params,
        vec!["x : bits(32)".to_string(), "y : int".to_string()]
    );
    assert_eq!(sig.return_type.as_deref(), Some("bits(32)"));
}

#[test]
fn caches_minimal_ast_for_file() {
    let file = File::new("function f(x : bits(32)) -> int = x".to_string());
    let ast = file.ast().expect("missing ast");
    assert!(!ast.items.is_empty());
}

#[test]
fn builds_signature_help_in_top_level_initializer() {
    let source = "val f : bits('n) -> bits('n)\nfunction f(x) = x\nlet _ = f(0xDEADBEEF)\n";
    let file = File::new(source.to_string());
    let uri = Url::parse("file:///tmp/main.sail").unwrap();
    let pos = file
        .source
        .position_at(source.find("0xDEADBEEF").unwrap() + 2);

    let help = signature_help_for_position(std::iter::once((&uri, &file)), &uri, &file, pos)
        .expect("signature help");
    assert_eq!(help.active_parameter, Some(0));
    assert_eq!(help.signatures.len(), 1);
    assert!(help.signatures[0].label.contains("bits('n) -> bits('n)"));
}

#[test]
fn finds_implementation_locations() {
    let file = File::new("val foo : int -> int\nfunction foo(x) = x\n".to_string());
    let uri = Url::parse("file:///tmp/test.sail").unwrap();
    let locations = implementation_locations(std::iter::once((&uri, &file)), &uri, "foo");
    assert!(!locations.is_empty());
}

#[test]
fn formats_document_indentation() {
    let options = FormattingOptions {
        tab_size: 2,
        insert_spaces: true,
        properties: HashMap::new(),
        trim_trailing_whitespace: Some(true),
        insert_final_newline: None,
        trim_final_newlines: None,
    };
    let source = "function f() = {\nlet x = [1,\n2]\n}\n";
    let formatted = format_document_text(source, &options);
    assert_eq!(formatted, "function f() = {\n  let x = [1,\n    2]\n}\n");
}

#[test]
fn does_not_count_braces_inside_comments_or_strings() {
    let options = FormattingOptions {
        tab_size: 2,
        insert_spaces: true,
        properties: HashMap::new(),
        trim_trailing_whitespace: Some(true),
        insert_final_newline: None,
        trim_final_newlines: None,
    };
    let source = "function f() = {\nlet x = \"}\" // {\n}\n";
    let formatted = format_document_text(source, &options);
    assert_eq!(formatted, "function f() = {\n  let x = \"}\" // {\n}\n");
}

#[test]
fn formats_only_selected_range() {
    let options = FormattingOptions {
        tab_size: 2,
        insert_spaces: true,
        properties: HashMap::new(),
        trim_trailing_whitespace: Some(true),
        insert_final_newline: None,
        trim_final_newlines: None,
    };
    let source = "function f() = {\nlet x = [1,\n2]\n}\n";
    let file = File::new(source.to_string());
    let edits = range_format_document_edits(
        &file,
        Range::new(
            tower_lsp::lsp_types::Position::new(1, 0),
            tower_lsp::lsp_types::Position::new(2, 5),
        ),
        &options,
    )
    .expect("expected range edit");
    assert_eq!(edits.len(), 1);
    assert_eq!(
        edits[0].range,
        Range::new(
            tower_lsp::lsp_types::Position::new(1, 0),
            tower_lsp::lsp_types::Position::new(3, 0),
        )
    );
    assert_eq!(edits[0].new_text, "  let x = [1,\n    2]\n");
}

#[test]
fn preserves_existing_continuation_indent() {
    let options = FormattingOptions {
        tab_size: 2,
        insert_spaces: true,
        properties: HashMap::new(),
        trim_trailing_whitespace: Some(true),
        insert_final_newline: None,
        trim_final_newlines: None,
    };
    let source = "mapping clause assembly = RFWVVTYPE(funct6, vm, vs2, vs1, vd)\n\t<-> rfwvvtype_mnemonic(funct6) ^ spc() ^ vreg_name(vd)\n";
    let formatted = format_document_text(source, &options);
    assert_eq!(formatted, source);
}

#[test]
fn preserves_tab_indent_even_when_computed_indent_is_spaces() {
    let options = FormattingOptions {
        tab_size: 2,
        insert_spaces: true,
        properties: HashMap::new(),
        trim_trailing_whitespace: Some(true),
        insert_final_newline: None,
        trim_final_newlines: None,
    };
    let source = "function f() = {\n\tx\n}\n";
    let formatted = format_document_text(source, &options);
    assert_eq!(formatted, source);
}

#[test]
fn does_not_indent_next_line_after_type_variables() {
    let options = FormattingOptions {
        tab_size: 2,
        insert_spaces: true,
        properties: HashMap::new(),
        trim_trailing_whitespace: Some(true),
        insert_final_newline: None,
        trim_final_newlines: None,
    };
    let source = "  let vm_val  : bits('n)             = read_vmask(num_elem_vs, vm, zvreg);\n  let vd_val  : vector('d, bits('m)) = read_vreg(num_elem_vd, SEW, 0, vd);\n";
    let formatted = format_document_text(source, &options);
    assert_eq!(formatted, source);
}

#[test]
fn returns_linked_editing_ranges_for_identifier() {
    let source = "let x = x\n";
    let file = File::new(source.to_string());
    let offset = source.rfind('x').expect("rhs x");
    let position = file.source.position_at(offset);
    let linked = linked_editing_ranges_for_position(&file, position).expect("linked ranges");
    assert!(linked.ranges.len() >= 2);
}

#[test]
fn extracts_document_links() {
    let uri = Url::parse("file:///tmp/main.sail").unwrap();
    let source = "let a = \"sub/module.sail\"\n// see https://example.com/spec\n";
    let file = File::new(source.to_string());
    let links = document_links_for_file(&uri, &file);
    assert!(links.len() >= 2);
    assert!(links.iter().any(|l| {
        l.target
            .as_ref()
            .map(|u| u.as_str().contains("example.com"))
            .unwrap_or(false)
    }));
}

#[test]
fn builds_code_lenses_for_declarations() {
    let source = "val foo : int\nfunction foo() = 1\n";
    let file = File::new(source.to_string());
    let uri = Url::parse("file:///tmp/main.sail").unwrap();
    let all_files = vec![(&uri, &file)];
    let refs = collect_reference_counts(&all_files);
    let impls = collect_implementation_counts(&all_files);
    let lenses = code_lenses_for_file(&file, &refs, &impls);
    assert!(lenses.len() >= 3);
    assert!(
        lenses
            .iter()
            .any(|lens| lens.command.is_some()
                && lens.command.as_ref().unwrap().title.contains("Run"))
    );
}

#[test]
fn builds_code_lens_title_from_data() {
    let refs = serde_json::json!({"kind":"refs","count":2});
    let impls = serde_json::json!({"kind":"impls","count":1});
    assert_eq!(code_lens_title(&refs).as_deref(), Some("2 references"));
    assert_eq!(code_lens_title(&impls).as_deref(), Some("1 implementation"));
}

#[test]
fn detects_unused_local_variables() {
    let source = "function foo() = {\n  let x = 1;\n  let y = 2;\n  y\n}\n";
    let file = File::new(source.to_string());
    let lsp_diagnostics = file.lsp_diagnostics();
    let unused_x = lsp_diagnostics
        .iter()
        .find(|d| d.message.contains("Unused variable: `x`"));
    let used_y = lsp_diagnostics
        .iter()
        .find(|d| d.message.contains("Unused variable: `y`"));

    assert!(unused_x.is_some());
    assert!(used_y.is_none());
    assert_eq!(
        unused_x.unwrap().severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::WARNING)
    );
    assert!(unused_x
        .unwrap()
        .tags
        .as_ref()
        .unwrap()
        .contains(&tower_lsp::lsp_types::DiagnosticTag::UNNECESSARY));
}

#[test]
fn detects_unused_shadowed_outer_binding() {
    let source = "function foo() = {\n  let x = 1;\n  let y = let x = 2 in x;\n  y\n}\n";
    let file = File::new(source.to_string());
    let unused_x = file
        .lsp_diagnostics()
        .into_iter()
        .filter(|diagnostic| diagnostic.message.contains("Unused variable: `x`"))
        .count();

    assert_eq!(unused_x, 1);
}

#[test]
fn detects_duplicate_definitions() {
    let source = "struct S = { x: int }\nstruct S = { y: int }\n";
    let file = File::new(source.to_string());
    let lsp_diagnostics = file.lsp_diagnostics();
    let dup = lsp_diagnostics
        .iter()
        .find(|d| d.message.contains("Duplicate definition of `S`"));

    assert!(dup.is_some());
    assert_eq!(
        dup.unwrap().severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
}

#[test]
fn detects_unreachable_code() {
    let source = "function foo() = {\n  return 1;\n  let x = 2;\n}\n";
    let file = File::new(source.to_string());
    let lsp_diagnostics = file.lsp_diagnostics();
    let unreachable = lsp_diagnostics
        .iter()
        .find(|d| d.message.contains("Unreachable code"));

    assert!(unreachable.is_some());
    assert_eq!(
        unreachable.unwrap().severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::HINT)
    );
    assert!(unreachable
        .unwrap()
        .tags
        .as_ref()
        .unwrap()
        .contains(&tower_lsp::lsp_types::DiagnosticTag::UNNECESSARY));
}

#[test]
fn detects_unreachable_after_terminating_if() {
    let source = "function foo(b) = {\n  if b then return 1 else return 2;\n  let x = 3;\n}\n";
    let file = File::new(source.to_string());
    let unreachable = file
        .lsp_diagnostics()
        .into_iter()
        .find(|diagnostic| diagnostic.message.contains("Unreachable code"));

    assert!(unreachable.is_some());
}

#[test]
fn detects_mismatched_argument_count() {
    let source = "val f : (int, int) -> int\nfunction f(a, b) = a + b\nlet _ = f(1)\n";
    let file = File::new(source.to_string());
    let lsp_diagnostics = file.lsp_diagnostics();
    let mismatch = lsp_diagnostics
        .iter()
        .find(|d| d.message.contains("Expected 2 arguments, found 1"));

    assert!(mismatch.is_some());
    assert_eq!(
        mismatch.unwrap().severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
}

#[test]
fn does_not_detect_duplicate_definitions_for_scattered_clauses() {
    let source = r#"
scattered function foo
function clause foo(x) = x
function clause foo(x) = x + 1
"#;
    let file = File::new(source.to_string());
    let lsp_diagnostics = file.lsp_diagnostics();
    let dup = lsp_diagnostics
        .iter()
        .find(|d| d.message.contains("Duplicate definition of `foo`"));

    assert!(dup.is_none());
}

#[test]
fn detects_mismatched_argument_count_with_implicits() {
    let source = "val f : (implicit(int), int) -> int\nfunction f(i, x) = x\nlet _ = f(1)\n";
    let file = File::new(source.to_string());
    let lsp_diagnostics = file.lsp_diagnostics();
    let mismatch = lsp_diagnostics
        .iter()
        .find(|d| d.message.contains("Expected"));

    // 1 argument is valid because 1 is implicit
    assert!(mismatch.is_none());

    let source2 = "val f : (implicit(int), int) -> int\nfunction f(i, x) = x\nlet _ = f(1, 2, 3)\n";
    let file2 = File::new(source2.to_string());
    let lsp_diagnostics2 = file2.lsp_diagnostics();
    let mismatch2 = lsp_diagnostics2
        .iter()
        .find(|d| d.message.contains("Expected 1-2 arguments, found 3"));
    assert!(mismatch2.is_some());
}

#[test]
fn handles_space_separated_params() {
    let source = "val HaveEL : bits(2) -> bool\nfunction HaveEL el = true\nlet _ = HaveEL(0b00)\n";
    let file = File::new(source.to_string());
    let lsp_diagnostics = file.lsp_diagnostics();
    let mismatch = lsp_diagnostics
        .iter()
        .find(|d| d.message.contains("Expected"));
    assert!(mismatch.is_none());
}

#[test]
fn finds_all_symbol_definition_locations_for_scattered_clauses() {
    let uri = Url::parse("file:///tmp/main.sail").unwrap();
    let source = r#"
scattered function foo
function clause foo(x) = x
function clause foo(x) = x + 1
"#;
    let file = File::new(source.to_string());
    let locations = symbol_definition_locations(std::iter::once((&uri, &file)), &uri, "foo");
    assert_eq!(locations.len(), 2);
}

#[test]
fn finds_symbol_declaration_locations_for_scattered_head() {
    let uri = Url::parse("file:///tmp/main.sail").unwrap();
    let source = r#"
scattered function foo
function clause foo(x) = x
"#;
    let file = File::new(source.to_string());
    let locations = symbol_declaration_locations(std::iter::once((&uri, &file)), &uri, "foo");
    assert_eq!(locations.len(), 1);
}

#[test]
fn counts_scattered_clauses_as_implementations() {
    let uri = Url::parse("file:///tmp/main.sail").unwrap();
    let source = r#"
scattered function foo
function clause foo(x) = x
function clause foo(x) = x + 1
"#;
    let file = File::new(source.to_string());
    let all_files = vec![(&uri, &file)];
    let impls = collect_implementation_counts(&all_files);

    assert_eq!(impls.get("foo").copied(), Some(2));
}

#[test]
fn resolves_workspace_symbol_location() {
    let uri = Url::parse("file:///tmp/main.sail").unwrap();
    let file = File::new("function foo() = 1\n".to_string());
    let symbol = WorkspaceSymbol {
        name: "foo".to_string(),
        kind: SymbolKind::FUNCTION,
        tags: None,
        container_name: None,
        location: OneOf::Right(WorkspaceLocation { uri: uri.clone() }),
        data: None,
    };
    let resolved = resolve_workspace_symbol(symbol, std::iter::once((&uri, &file)));
    assert!(matches!(resolved.location, OneOf::Left(_)));
}

#[test]
fn extracts_type_alias_edges() {
    let file = File::new("type child = parent\n".to_string());
    let edges = type_alias_edges(&file);
    assert_eq!(edges, vec![("child".to_string(), "parent".to_string())]);
}

#[test]
fn computes_type_hierarchy_relations() {
    let uri = Url::parse("file:///tmp/main.sail").unwrap();
    let file =
        File::new("type parent = base\ntype child = parent\ntype grandchild = child\n".to_string());
    let supers = type_supertypes(std::iter::once((&uri, &file)), &uri, "child");
    let subs = type_subtypes(std::iter::once((&uri, &file)), &uri, "child");
    assert!(supers.iter().any(|item| item.name == "parent"));
    assert!(subs.iter().any(|item| item.name == "grandchild"));
}

#[test]
fn finds_type_candidates_at_position() {
    let source = "let x : child = y\n";
    let file = File::new(source.to_string());
    let pos = file.source.position_at(source.find("x").unwrap());
    let names = type_name_candidates_at_position(&file, pos);
    assert!(names.contains(&"child".to_string()));
    assert!(names.contains(&"x".to_string()));
}

#[test]
fn builds_document_diagnostic_report_and_unchanged() {
    let file = File::new("let x =\n".to_string());
    assert!(file.parsed().is_some());
    let full = document_diagnostic_report_for_file(&file, None);
    let result_id = match full {
        DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Full(report)) => report
            .full_document_diagnostic_report
            .result_id
            .expect("result id"),
        _ => panic!("expected full report"),
    };
    let unchanged = document_diagnostic_report_for_file(&file, Some(&result_id));
    assert!(matches!(
        unchanged,
        DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Unchanged(_))
    ));
}

#[test]
fn builds_workspace_diagnostic_report() {
    let uri = Url::parse("file:///tmp/main.sail").unwrap();
    let file = File::new("let x =\n".to_string());
    let mut versions = HashMap::new();
    versions.insert(uri.clone(), 3);
    let report =
        workspace_diagnostic_report(std::iter::once((&uri, &file)), &versions, &HashMap::new());
    match report {
        WorkspaceDiagnosticReportResult::Report(report) => {
            assert_eq!(report.items.len(), 1);
        }
        _ => panic!("expected full workspace report"),
    }
}

#[test]
fn creates_will_rename_file_edits() {
    let uri = Url::parse("file:///tmp/main.sail").unwrap();
    let file = File::new("let inc = \"old.sail\"\n".to_string());
    let params = RenameFilesParams {
        files: vec![tower_lsp::lsp_types::FileRename {
            old_uri: "file:///tmp/old.sail".to_string(),
            new_uri: "file:///tmp/new.sail".to_string(),
        }],
    };
    let changes =
        will_rename_file_edits(std::iter::once((&uri, &file)), &params).expect("expected edits");
    assert_eq!(changes.len(), 1);
}

#[test]
fn lazy_code_action_data_roundtrip() {
    let uri = Url::parse("file:///tmp/main.sail").unwrap();
    let edit = TextEdit {
        range: Range::new(
            tower_lsp::lsp_types::Position::new(0, 0),
            tower_lsp::lsp_types::Position::new(0, 0),
        ),
        new_text: ";".to_string(),
    };
    let data = lazy_code_action_data(&uri, std::slice::from_ref(&edit));
    let (decoded_uri, decoded_edits) = resolve_code_action_edit_from_data(&data).expect("decode");
    assert_eq!(decoded_uri, uri);
    assert_eq!(decoded_edits, vec![edit]);
}

#[test]
fn code_action_kind_filter_matches_prefixes() {
    let requested = Some(vec![CodeActionKind::REFACTOR]);
    assert!(code_action_kind_allowed(
        &requested,
        &CodeActionKind::REFACTOR_REWRITE
    ));
    assert!(!code_action_kind_allowed(
        &requested,
        &CodeActionKind::QUICKFIX
    ));
}

#[test]
fn code_action_kind_filter_matches_custom_source_fix_all() {
    let requested = Some(vec![CodeActionKind::SOURCE_FIX_ALL]);
    assert!(code_action_kind_allowed(
        &requested,
        &sail_source_fix_all_kind()
    ));
    assert!(!code_action_kind_allowed(
        &Some(vec![CodeActionKind::REFACTOR]),
        &sail_source_fix_all_kind()
    ));
}

#[test]
fn resolves_local_symbol_occurrences_without_crossing_shadowing_scopes() {
    let source = "function foo() = {\n  let x = 1;\n  let y = let x = 2 in x;\n  x + y\n}\n";
    let file = File::new(source.to_string());
    assert!(file.ast().is_some());
    let pos = file
        .source
        .position_at(source.rfind("x + y").expect("outer x"));

    let symbol = resolve_symbol_at(&file, pos).expect("resolved symbol");
    let spans = symbol_spans_for_file(&file, &symbol, true);

    assert_eq!(spans.len(), 2);
    assert!(spans.iter().any(|(span, is_write)| {
        *is_write
            && &source[span.start..span.end] == "x"
            && span.start < source.find("let y").unwrap()
    }));
    assert!(spans.iter().any(|(span, is_write)| {
        !*is_write
            && &source[span.start..span.end] == "x"
            && span.start > source.find("let y").unwrap()
    }));
}

#[test]
fn resolves_match_pattern_bindings_via_ast_symbol_occurrences() {
    let source = "function foo(xs) = match xs {\n  Some(x) => x,\n  None() => 0\n}\n";
    let file = File::new(source.to_string());
    assert!(file.ast().is_some());
    let pos = file
        .source
        .position_at(source.rfind("=> x").expect("body x") + 3);

    let symbol = resolve_symbol_at(&file, pos).expect("resolved symbol");
    let spans = symbol_spans_for_file(&file, &symbol, true);

    assert_eq!(spans.len(), 2);
    assert!(spans
        .iter()
        .any(|(span, is_write)| *is_write && &source[span.start..span.end] == "x"));
    assert!(spans
        .iter()
        .any(|(span, is_write)| !*is_write && &source[span.start..span.end] == "x"));
}

#[test]
fn top_level_references_ignore_shadowed_local_bindings() {
    let uri1 = Url::parse("file:///tmp/a.sail").unwrap();
    let uri2 = Url::parse("file:///tmp/b.sail").unwrap();
    let source1 = "val foo : unit -> int\nfunction foo() = 1\nfunction use_foo() = foo()\n";
    let source2 = "function bar() = {\n  let foo = 1;\n  foo\n}\n";
    let file1 = File::new(source1.to_string());
    let file2 = File::new(source2.to_string());
    assert!(file1.ast().is_some());
    assert!(file2.ast().is_some());
    let pos = file1
        .source
        .position_at(source1.find("foo() = 1").expect("foo definition"));

    let symbol = resolve_symbol_at(&file1, pos).expect("resolved symbol");
    let locations =
        reference_locations(vec![(&uri1, &file1), (&uri2, &file2)], &uri1, &symbol, true);

    assert_eq!(locations.len(), 3);
    assert!(locations.iter().all(|location| location.uri == uri1));
}

#[test]
fn renames_type_variables_within_their_own_scope_only() {
    let uri1 = Url::parse("file:///tmp/a.sail").unwrap();
    let uri2 = Url::parse("file:///tmp/b.sail").unwrap();
    let source1 = "val f : forall ('n). bits('n) -> bits('n)\n";
    let source2 = "val g : forall ('n). bits('n) -> bits('n)\n";
    let file1 = File::new(source1.to_string());
    let file2 = File::new(source2.to_string());
    assert!(file1.ast().is_some());
    assert!(file2.ast().is_some());
    let pos = file1
        .source
        .position_at(source1.find("'n").expect("type var"));

    let symbol = resolve_symbol_at(&file1, pos).expect("resolved symbol");
    let changes = rename_edits(vec![(&uri1, &file1), (&uri2, &file2)], &uri1, &symbol, "'m");

    assert_eq!(changes.len(), 1);
    assert_eq!(changes.get(&uri1).map(Vec::len), Some(3));
    assert!(!changes.contains_key(&uri2));
}

#[test]
fn completion_uses_ast_scoped_bindings_for_local_candidates() {
    let uri = Url::parse("file:///tmp/main.sail").unwrap();
    let source = "function foo() = {\n  let local_value = 1;\n  local_\n}\n";
    let file = File::new(source.to_string());
    let offset = source.find("local_\n").expect("completion site") + "local_".len();
    let prefix = completion_prefix(file.source.text(), offset);
    let items = build_completion_items(
        std::iter::once((&uri, &file)),
        &uri,
        file.source.text(),
        offset,
        prefix,
        SAIL_KEYWORDS,
        SAIL_BUILTINS,
    );

    assert!(items.iter().any(|item| item.label == "local_value"));
}
