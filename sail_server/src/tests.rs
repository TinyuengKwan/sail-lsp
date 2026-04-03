use super::Parameter;
use super::*;
use tower_lsp::lsp_types::{Diagnostic, NumberOrString};

fn diagnostic_code_str(diagnostic: &tower_lsp::lsp_types::Diagnostic) -> Option<&str> {
    match diagnostic.code.as_ref()? {
        NumberOrString::String(code) => Some(code.as_str()),
        NumberOrString::Number(_) => None,
    }
}

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
fn reports_lexical_errors_with_lexical_error_code() {
    let file = File::new("let _ = ™;\n".to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| diagnostic_code_str(diagnostic) == Some("lexical-error"))
        .expect("missing lexical diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
}

#[test]
fn reports_missing_closing_paren_as_syntax_error() {
    let source = "function f() = {\n  let x = (1 + 2\n}\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("syntax-error")
                && diagnostic.message.contains("expected ')'")
        })
        .expect("missing syntax diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
}

#[test]
fn reports_type_errors_from_typecheck() {
    let source = "val f : bool -> unit\nfunction f(x) = ()\nfunction g() = f(1)\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| diagnostic_code_str(diagnostic) == Some("type-error"))
        .expect("missing type diagnostic");

    assert!(diagnostic.message.contains("bool"));
}

#[test]
fn infers_concat_chain_type_from_typecheck() {
    let source = "function f(x : bits(3), y : bits(2), z : bits(1)) = x @ y @ z\n";
    let file = File::new(source.to_string());
    let ast = file.core_ast().expect("core ast");
    let type_check = file.type_check().expect("typecheck result");
    let body = ast
        .defs
        .iter()
        .find_map(|(def, _)| match &def.kind {
            sail_parser::core_ast::DefinitionKind::Callable(def) => def.body.as_ref(),
            _ => None,
        })
        .expect("callable body");
    assert_eq!(&source[body.1.start..body.1.end], "x @ y @ z");
    assert_eq!(type_check.expr_type_text(body.1), Some("bits(6)"));
}

#[test]
fn reports_mismatched_arg_count_from_typecheck() {
    let source =
        "val add : (int, int) -> int\nfunction add(x, y) = x + y\nfunction main() = add(1)\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| diagnostic_code_str(diagnostic) == Some("mismatched-arg-count"))
        .expect("missing mismatched arg count diagnostic");

    assert!(diagnostic.message.contains("Expected 2 arguments, found 1"));
}

#[test]
fn reports_missing_record_fields_from_typecheck() {
    let source = "struct S = { x : int, y : bool }\nfunction f() = { struct S { x = 1 } }\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic
                    .message
                    .contains("struct literal missing fields: y")
        })
        .expect("missing record field diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
}

#[test]
fn reports_record_field_type_errors_from_typecheck() {
    let source = "struct S = { x : int }\nfunction f() = { struct S { x = true } }\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("bool")
                && diagnostic.message.contains("int")
        })
        .expect("missing record field type diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
}

#[test]
fn reports_record_update_field_type_errors_from_typecheck() {
    let source =
        "struct S = { x : int }\nlet s : S = struct S { x = 1 }\nlet _ = { s with x = true }\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("bool")
                && diagnostic.message.contains("int")
        })
        .expect("missing record update diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
}

#[test]
fn applies_record_type_arguments_to_field_types() {
    let source = "struct pair('a) = { fst : 'a, snd : 'a }\nfunction f(p : pair(int)) = { let x : bool = p.fst; x }\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("int")
                && diagnostic.message.contains("bool")
        })
        .expect("missing instantiated record field diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
}

#[test]
fn applies_expected_record_type_to_generic_struct_literals() {
    let source = "struct pair('a) = { fst : 'a, snd : 'a }\nfunction mk() -> pair(int) = { struct pair { fst = 1, snd = true } }\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("bool")
                && diagnostic.message.contains("int")
        })
        .expect("missing generic struct literal diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
}

#[test]
fn reports_list_element_type_errors_against_expected_return_type() {
    let source = "function f() -> list(bool) = [|true, 1|]\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("int")
                && diagnostic.message.contains("bool")
        })
        .expect("missing list element type diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
}

#[test]
fn reports_tuple_element_type_errors_against_expected_return_type() {
    let source = "function f() -> (int, bool) = (1, 2)\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("int")
                && diagnostic.message.contains("bool")
        })
        .expect("missing tuple element type diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
}

#[test]
fn reports_vector_length_errors_against_expected_return_type() {
    let source = "function f() -> vector(2, bool) = [true, false, true]\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("vector(3")
                && diagnostic.message.contains("vector(2, bool)")
        })
        .expect("missing vector length diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
}

#[test]
fn binds_union_constructor_pattern_payload_types() {
    let source = "union opt('a) = { None : unit, Some : 'a }\nfunction f(x : opt(int)) = match x { Some(v) => if v then () else (), None() => () }\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("int")
                && diagnostic.message.contains("bool")
        })
        .expect("missing constructor payload type diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
    assert!(diagnostics
        .iter()
        .all(|diagnostic| !diagnostic.message.contains("Identifier v is unbound")));
}

#[test]
fn checks_match_case_bodies_against_expected_return_type() {
    let source = "union opt('a) = { None : unit, Some : 'a }\nfunction f(x : opt(int)) -> bool = match x { Some(v) => v, None() => false }\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("int")
                && diagnostic.message.contains("bool")
        })
        .expect("missing match branch expected type diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
}

#[test]
fn typechecks_union_constructor_calls_as_expressions() {
    let source =
        "union opt('a) = { None : unit, Some : 'a }\nfunction f() -> opt(int) = Some(true)\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("bool")
                && diagnostic.message.contains("int")
        })
        .expect("missing constructor call type diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
}

#[test]
fn checks_annotated_local_list_bindings_with_expected_type() {
    let source = "function f() = { let xs : list(bool) = [|true, 1|]; () }\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("int")
                && diagnostic.message.contains("bool")
        })
        .expect("missing annotated let type diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
}

#[test]
fn reports_struct_pattern_missing_fields_from_typecheck() {
    let source =
        "struct S = { x : int, y : bool }\nfunction f(s : S) = match s { struct S { x } => () }\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic
                    .message
                    .contains("struct pattern missing fields: y")
        })
        .expect("missing struct pattern field diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
}

#[test]
fn does_not_report_missing_fields_for_struct_pattern_wildcard() {
    let source = "struct S = { x : int, y : bool }\nfunction f(s : S) = match s { struct S { x, _ } => x }\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics
        .iter()
        .all(|diagnostic| !diagnostic.message.contains("struct pattern missing fields")));
}

#[test]
fn reports_duplicate_pattern_bindings_from_typecheck() {
    let source = "function f(xs : list(int)) = match xs { x :: x => x, [||] => 0 }\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic
                    .message
                    .contains("Duplicate binding for x in pattern")
        })
        .expect("missing duplicate pattern diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
}

#[test]
fn binds_vector_subrange_patterns_like_upstream() {
    let source = "default Order dec\nfunction f(x : bits(8)) = match x { flag[7 .. 4] @ flag[3 .. 0] => if flag then () else () }\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("bits(8)")
                && diagnostic.message.contains("bool")
        })
        .expect("missing vector subrange binding diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
    assert!(diagnostics
        .iter()
        .all(|diagnostic| !diagnostic.message.contains("Identifier flag is unbound")));
}

#[test]
fn binds_multi_part_vector_subrange_patterns_like_upstream() {
    let source = "default Order dec\nfunction f(x : bits(8)) = match x { flag[7 .. 6] @ flag[5 .. 2] @ flag[1 .. 0] => if flag then () else () }\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("bits(8)")
                && diagnostic.message.contains("bool")
        })
        .expect("missing vector subrange binding diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
    assert!(diagnostics
        .iter()
        .all(|diagnostic| !diagnostic.message.contains("Identifier flag is unbound")));
}

#[test]
fn reports_non_contiguous_vector_subrange_patterns() {
    let source = "default Order dec\nfunction f(x : bits(8)) = match x { flag[7 .. 4] @ flag[2 .. 0] => (), _ => () }\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic
                    .message
                    .contains("pattern subranges are non-contiguous")
        })
        .expect("missing non-contiguous subrange diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
}

#[test]
fn typechecks_mapping_calls_as_expressions() {
    let source = "enum width = BYTE | DOUBLE\nmapping size_bits : width <-> bits(2) = { BYTE <-> 0b00, DOUBLE <-> 0b11 }\nfunction f() -> bits(2) = size_bits(BYTE)\nfunction g() -> width = size_bits(0b11)\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic_code_str(diagnostic) != Some("type-error")));
}

#[test]
fn binds_mapping_pattern_payload_types() {
    let source = "enum width = BYTE | DOUBLE\nmapping size_bits : width <-> bits(2) = { BYTE <-> 0b00, DOUBLE <-> 0b11 }\nfunction f(x : width) = match x { size_bits(bits) => if bits then () else () }\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("bits(2)")
                && diagnostic.message.contains("bool")
        })
        .expect("missing mapping pattern payload diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
    assert!(diagnostics
        .iter()
        .all(|diagnostic| !diagnostic.message.contains("Identifier bits is unbound")));
}

#[test]
fn checks_mapping_guards_with_pattern_bindings() {
    let source = "enum width = BYTE | DOUBLE\nval decode : bits(2) -> width\nmapping size_bits : width <-> bits(2) = { backwards bits if bits => decode(bits) }\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("bits(2)")
                && diagnostic.message.contains("bool")
        })
        .expect("missing mapping guard diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
    assert!(diagnostics
        .iter()
        .all(|diagnostic| !diagnostic.message.contains("Identifier bits is unbound")));
}

#[test]
fn reports_bidirectional_mapping_binding_mismatches() {
    let source = "mapping same : bits(2) <-> bits(2) = { left <-> right }\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic_code_str(diagnostic) == Some("type-error")
            && diagnostic
                .message
                .contains("Identifier left found on left hand side of mapping, but not on right")
    }));
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic_code_str(diagnostic) == Some("type-error")
            && diagnostic
                .message
                .contains("Identifier right found on right hand side of mapping, but not on left")
    }));
}

#[test]
fn reports_unresolved_quants_from_generic_call_without_context() {
    let source = "val zeroes : forall 'n. unit -> bits('n)\nfunction use() = zeroes(())\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic
                    .message
                    .contains("Could not resolve quantifiers for zeroes")
        })
        .expect("missing unresolved quantifier diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
}

#[test]
fn reports_failed_constraints_from_generic_call() {
    let source =
        "val widen : forall 'n, 'n in {1, 2}. unit -> bits(8 * 'n)\nfunction use() -> bits(24) = widen(())\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("Failed to prove constraint")
                && diagnostic.message.contains("'n in {1, 2}")
        })
        .expect("missing failed constraint diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
}

#[test]
fn infers_slice_result_types_for_bits() {
    let source = "default Order dec\nfunction hi(x : bits(8)) -> bits(4) = x[7 .. 4]\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic_code_str(diagnostic) != Some("type-error")));
}

#[test]
fn typechecks_comma_slice_sugar_via_builtin_slice() {
    let source = "function mid(x : bits(8)) -> bits(3) = x[2, 3]\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic_code_str(diagnostic) != Some("type-error")));
}

#[test]
fn reports_out_of_bounds_bit_index_access() {
    let source = "function bit_at(x : bits(8)) -> bit = x[8]\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic_code_str(diagnostic) == Some("type-error")
            && diagnostic.message.contains("Failed to prove constraint")
            && diagnostic.message.contains("0 <= 8 < 8")
    }));
}

#[test]
fn reports_vector_update_range_value_type_mismatch() {
    let source =
        "default Order dec\nfunction patch(x : bits(8)) -> bits(8) = [x with 7 .. 4 = 0b101]\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic_code_str(diagnostic) == Some("type-error")
            && diagnostic.message.contains("bits(3)")
            && diagnostic.message.contains("bits(4)")
    }));
}

#[test]
fn treats_ref_register_as_register_typed_expression() {
    let source = "val reg_deref : forall ('a : Type). register('a) -> 'a\nregister R : bits(8)\nfunction read() -> bits(8) = reg_deref(ref R)\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic_code_str(diagnostic) != Some("type-error")));
}

#[test]
fn infers_bitfield_access_and_updates() {
    let source = "bitfield B : bits(8) = { HI : 7 .. 4, LO : 3 }\nregister R : B\nfunction hi() -> bits(4) = R[HI]\nfunction lo() -> bit = R.LO\nfunction bits() -> bits(8) = R.bits\nfunction patch() -> B = [R with HI = 0b1010]\nfunction write() = { R[HI] = 0b0001; () }\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic_code_str(diagnostic) != Some("type-error")));
}

#[test]
fn infers_concat_bitfield_ranges_like_upstream() {
    let source = "bitfield B : bits(32) = { Field0 : (31 .. 16 @ 7 .. 0), Field1 : 15 .. 8 }\nregister R : B\nfunction get0() -> bits(24) = R[Field0]\nfunction get1() -> bits(8) = R[Field1]\nfunction patch() -> B = [R with Field1 = 0x47, Field0 = 0x000011]\nfunction write() = { R[Field0] = 0x4711FF; () }\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic_code_str(diagnostic) != Some("type-error")));
}

#[test]
fn supports_arrow_modifier_calls_for_register_overloads() {
    let source = "val _get_hi : register(bits(8)) -> bits(4)\nval _set_hi : (register(bits(8)), bits(4)) -> unit\noverload _mod_hi = {_get_hi, _set_hi}\nregister R : bits(8)\nfunction read() -> bits(4) = R->hi()\nfunction write() = { R->hi() = 0b1010; () }\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic_code_str(diagnostic) != Some("type-error")));
}

#[test]
fn uses_expected_return_to_resolve_generic_call_quantifiers() {
    let source =
        "val zeroes : forall 'n. unit -> bits('n)\nfunction use() -> bits(8) = zeroes(())\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic_code_str(diagnostic) != Some("type-error")));
}

#[test]
fn solves_linear_numeric_quants_from_expected_return_type() {
    let source =
        "val widen : forall 'n. unit -> bits(8 * 'n)\nfunction use() -> bits(16) = widen(())\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic_code_str(diagnostic) != Some("type-error")));
}

#[test]
fn reports_unresolved_quants_for_unproved_symbolic_constraints() {
    let source = "val take_width : forall 'n, 0 < 'n <= max_mem_access . bits('n) -> unit\nfunction use() = take_width(0b10101010)\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic_code_str(diagnostic) == Some("type-error")
            && diagnostic.message.contains("0 < 'n <= max_mem_access")
    }));
}

#[test]
fn resolves_quant_constraints_via_global_constraint_definitions() {
    let source = "type max_mem_access : Int\nconstraint max_mem_access == 8\nval take_width : forall 'n, 0 < 'n <= max_mem_access . bits('n) -> unit\nfunction use() = take_width(0b10101010)\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic_code_str(diagnostic) != Some("type-error")));
}

#[test]
fn propagates_assert_constraints_to_following_calls() {
    let source = "let max_mem_access : int = 8\nval take_width : forall 'n, 0 < 'n <= max_mem_access . bits('n) -> unit\nfunction use() = { assert(max_mem_access == 8, \"ok\"); take_width(0b10101010) }\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic_code_str(diagnostic) != Some("type-error")));
}

#[test]
fn resolves_quant_constraints_from_matching_global_assumptions() {
    let source = "type xlen : Int\nconstraint xlen in {32, 64}\nval take_width : forall 'n, 'n in {32, 64} . bits('n) -> unit\nfunction use(xs : bits(xlen)) = take_width(xs)\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic_code_str(diagnostic) != Some("type-error")));
}

#[test]
fn resolves_weaker_quant_bounds_from_global_set_constraints() {
    let source = "type xlen : Int\nconstraint xlen in {32, 64}\nval take_width : forall 'n, 1 <= 'n <= 64 . bits('n) -> unit\nfunction use(xs : bits(xlen)) = take_width(xs)\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic_code_str(diagnostic) != Some("type-error")));
}

#[test]
fn reports_inconsistent_global_constraints() {
    let source = "type xlen : Int\nconstraint xlen == 32\nconstraint xlen == 64\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic_code_str(diagnostic) == Some("type-error")
            && diagnostic
                .message
                .contains("Global constraint appears inconsistent with previous global constraints")
    }));
}

#[test]
fn reports_literal_pattern_type_mismatches() {
    let source = "function f(x : string) = match x { 1 => (), _ => () }\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("int")
                && diagnostic.message.contains("string")
        })
        .expect("missing literal pattern mismatch diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
}

#[test]
fn infers_callable_head_param_types_from_literal_patterns() {
    let source = "function pick((1, x : int)) -> int = x\nfunction use() = pick((true, 0))\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("bool")
                && diagnostic.message.contains("int")
        })
        .expect("missing inferred callable head type diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
}

#[test]
fn reports_non_int_index_from_typecheck() {
    let source = "function f() = { let v = [1, 2]; v[true] }\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("bool")
                && diagnostic.message.contains("int")
        })
        .expect("missing index diagnostic");

    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
}

#[test]
fn binds_foreach_iterators_for_typecheck() {
    let source = "function f(n : int) = { foreach (i from 0 to n) { if i then () else () }; () }\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let bool_mismatch = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("int")
                && diagnostic.message.contains("bool")
        })
        .expect("missing foreach iterator type diagnostic");

    assert_eq!(
        bool_mismatch.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::ERROR)
    );
    assert!(diagnostics
        .iter()
        .all(|diagnostic| !diagnostic.message.contains("Identifier i is unbound")));
}

#[test]
fn warns_on_upstream_deprecated_effect_annotations() {
    let source = "val write_ram : unit -> bool effect {wmv}\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| diagnostic_code_str(diagnostic) == Some("deprecated-effect-annotation"))
        .expect("missing effect warning");

    assert_eq!(
        diagnostic.message,
        "Explicit effect annotations are deprecated. They are no longer used and can be removed."
    );
    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::WARNING)
    );
}

#[test]
fn warns_when_extern_purity_is_missing() {
    let source = "val trace_name = {lem: \"trace_name\"} : unit -> unit\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| diagnostic_code_str(diagnostic) == Some("missing-extern-purity"))
        .expect("missing extern purity warning");

    assert_eq!(
        diagnostic.message,
        "All external bindings should be marked as either pure or impure"
    );
    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::WARNING)
    );
}

#[test]
fn warns_on_upstream_deprecated_cast_annotations() {
    let source = "function f(x) = x : int\n";
    let file = File::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| diagnostic_code_str(diagnostic) == Some("deprecated-cast-annotation"))
        .expect("missing cast warning");

    assert_eq!(
        diagnostic.message,
        "Cast annotations are deprecated. They will be removed in a future version of the language."
    );
    assert_eq!(
        diagnostic.severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::WARNING)
    );
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
fn infers_unannotated_binding_types_from_typecheck() {
    let file = File::new("function f() = { let x = 1; x }".to_string());
    let bindings = typed_bindings(&file);
    assert_eq!(bindings.get("x"), Some(&"int".to_string()));
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
fn caches_core_ast_for_file() {
    let file = File::new("function f(x : bits(32)) -> int = x".to_string());
    let core_ast = file.core_ast().expect("missing core ast");
    assert!(!core_ast.defs.is_empty());
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
fn does_not_warn_for_enum_members_in_patterns() {
    let source = "enum instr = VI_VRGATHER | VI_VADD\nfunction decode(instr) = match instr {\n  VI_VRGATHER => 1,\n  VI_VADD => 2\n}\n";
    let file = File::new(source.to_string());
    let lsp_diagnostics = file.lsp_diagnostics();

    assert!(!lsp_diagnostics
        .iter()
        .any(|d| d.message.contains("Unused variable: `VI_VRGATHER`")));
    assert!(!lsp_diagnostics
        .iter()
        .any(|d| d.message.contains("Unused variable: `VI_VADD`")));
}

#[test]
fn resolves_enum_member_patterns_as_top_level_symbols() {
    let source = "enum instr = VI_VRGATHER | VI_VADD\nfunction decode(instr) = match instr {\n  VI_VRGATHER => 1,\n  VI_VADD => 2\n}\n";
    let file = File::new(source.to_string());
    let pos = file
        .source
        .position_at(source.rfind("VI_VRGATHER =>").expect("pattern ref"));

    let symbol = resolve_symbol_at(&file, pos).expect("resolved symbol");
    assert_eq!(symbol.scope, Some(sail_parser::Scope::TopLevel));
    assert_eq!(symbol.target_span, None);

    let spans = symbol_spans_for_file(&file, &symbol, true);
    assert_eq!(spans.len(), 2);
    assert!(spans
        .iter()
        .any(|(span, is_write)| { *is_write && &source[span.start..span.end] == "VI_VRGATHER" }));
    assert!(spans
        .iter()
        .any(|(span, is_write)| { !*is_write && &source[span.start..span.end] == "VI_VRGATHER" }));
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
    assert!(file.core_ast().is_some());
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
    assert!(file.core_ast().is_some());
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
    assert!(file1.core_ast().is_some());
    assert!(file2.core_ast().is_some());
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
    assert!(file1.core_ast().is_some());
    assert!(file2.core_ast().is_some());
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
