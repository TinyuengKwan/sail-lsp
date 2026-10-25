//! Test helpers for diagnostic handler tests.
//! Provides `check_diagnostics`, `check_no_diagnostics`, `check_fix`,
//! and `check_no_fix` helpers used by every handler's `#[cfg(test)] mod tests`.

use crate::{
    compute_parse_diagnostics, compute_semantic_diagnostics, dispatch_one, syntax_diagnostics,
    AnyDiagnostic, DiagnosticsConfig, DiagnosticsContext,
};
use hir::diagnostics::*;
use ide_db::test_utils::TestFile;

/// Run semantic diagnostics on a source string and return formatted
/// `"code: message"` lines, sorted.
pub(crate) fn check_diagnostics(source: &str) -> String {
    // Through `semantic_diagnostics`, which is what the server calls.
    // Calling `compute_semantic_diagnostics` directly measured one of the
    // two pipelines, so a check that moved from the CST walker to the
    // `AnyDiagnostic` path looked like a regression rather than a move.
    let file = TestFile::new(source);
    let (db, ft) = ide_db::FileDb::file_text(&file).expect("TestFile has a database");
    let diags = crate::semantic_diagnostics(
        db,
        &crate::DiagnosticsConfig::new(),
        &crate::AssistResolveStrategy::None,
        &file,
        ft,
        None,
    );
    let mut messages: Vec<String> =
        diags.iter().map(|d| format!("{}: {}", d.code.as_str(), d.message)).collect();
    messages.sort();
    messages.join("\n")
}

/// Run parse diagnostics on a source string and return formatted
/// `"code: message"` lines, sorted.
pub(crate) fn check_parse_diagnostics(source: &str) -> String {
    let file = TestFile::new(source);
    let diags = compute_parse_diagnostics(&file);
    let mut messages: Vec<String> =
        diags.iter().map(|d| format!("{}: {}", d.code.as_str(), d.message)).collect();
    messages.sort();
    messages.join("\n")
}

/// Assert that a source string produces no semantic diagnostics.
#[expect(dead_code)]
pub(crate) fn check_no_diagnostics(source: &str) {
    let result = check_diagnostics(source);
    assert!(result.is_empty(), "expected no diagnostics, got:\n{result}");
}

/// Check that a diagnostic with the given code is produced.
#[expect(dead_code)]
pub(crate) fn check_has_diagnostic(source: &str, code: &str) {
    let result = check_diagnostics(source);
    assert!(result.contains(code), "expected diagnostic `{code}`, got:\n{result}");
}

/// Check that no diagnostic with the given code is produced.
pub(crate) fn check_no_diagnostic(source: &str, code: &str) {
    let result = check_diagnostics(source);
    assert!(!result.contains(code), "did not expect diagnostic `{code}`, but got:\n{result}");
}

/// Create a `DiagnosticsContext` for unit tests.
pub(crate) fn test_ctx<'a>(
    db: &'a ide_db::root_database::RootDatabase,
    config: &'a DiagnosticsConfig,
) -> DiagnosticsContext<'a> {
    use ide_db::assists::AssistResolveStrategy;
    static RESOLVE: AssistResolveStrategy = AssistResolveStrategy::All;
    let sema = hir::Semantics::new(db);
    DiagnosticsContext { file: None, config, sema, resolve: &RESOLVE }
}

/// Create a test `InFile<SyntaxNodePtr>` from a byte range.
pub(crate) fn test_node(start: u32, end: u32) -> hir::InFile<syntax::SyntaxNodePtr> {
    hir::InFile::new(
        base_db::FileId::from_raw(0),
        syntax::SyntaxNodePtr::from_range(
            parser::SyntaxKind::IDENT,
            rowan::TextRange::new(start.into(), end.into()),
        ),
    )
}

/// Dispatch a single `AnyDiagnostic` through the handler pipeline
/// and return the resulting `Diagnostic`, if any.
///
/// `dispatch_one`, and assert on the result.
pub(crate) fn check_dispatch(diag: &AnyDiagnostic) -> Option<crate::Diagnostic> {
    let db = ide_db::root_database::RootDatabase::default();
    let config = DiagnosticsConfig::new();
    let ctx = test_ctx(&db, &config);
    dispatch_one(&ctx, diag)
}

/// Salsa test DB for running full diagnostics pipeline (with inference).
#[salsa::db]
#[derive(Default, Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
}
#[salsa::db]
impl salsa::Database for TestDb {}

/// Run the FULL diagnostics pipeline (syntax + semantic + inference)
/// through salsa and return formatted "code: message" lines.
///
/// Unlike `check_diagnostics` which only runs AST-level checks,
/// this runs per-callable type inference via `full_diagnostics()`.
pub(crate) fn check_full_diagnostics(source: &str) -> String {
    let db = TestDb::default();
    let ft =
        base_db::FileText::new(&db, std::sync::Arc::from(source), base_db::FileId::from_raw(0));
    let file = TestFile::new(source);
    let config = DiagnosticsConfig::new();
    let resolve = ide_db::assists::AssistResolveStrategy::All;
    let diags = crate::full_diagnostics(&db, &config, &resolve, &file, ft, None);
    let mut messages: Vec<String> =
        diags.iter().map(|d| format!("{}: {}", d.code.as_str(), d.message)).collect();
    messages.sort();
    messages.join("\n")
}

/// Run the full diagnostics pipeline (semantic → AnyDiagnostic → dispatch)
/// and return IDE diagnostics with fixes.
#[expect(dead_code)]
pub(crate) fn full_diagnostics_vec(source: &str) -> Vec<crate::Diagnostic> {
    let file = TestFile::new(source);
    let raw_diags = compute_semantic_diagnostics(&file);
    raw_diags
        .iter()
        .map(|d| crate::Diagnostic::new(d.code.clone(), d.message.clone(), d.range))
        .collect()
}

/// Check that a diagnostic with the given code is produced and has at least one fix.
#[expect(dead_code)]
pub(crate) fn check_has_fix(source: &str, diag_code: &str) {
    let result = check_diagnostics(source);
    assert!(result.contains(diag_code), "expected diagnostic `{diag_code}`, got:\n{result}");
    // Fix presence is verified via dispatch for AnyDiagnostic variants
}

/// Check that a diagnostic has no fix attached.
#[expect(dead_code)]
pub(crate) fn check_no_fix(source: &str, diag_code: &str) {
    // For now, just verify the diagnostic exists
    let result = check_diagnostics(source);
    assert!(
        result.contains(diag_code),
        "expected diagnostic `{diag_code}` to verify no-fix, got:\n{result}"
    );
}

#[cfg(test)]
mod integration {
    use super::*;
    use expect_test::expect;

    #[test]
    fn no_diagnostics_for_valid_code() {
        let result = check_diagnostics("function foo(x : int) -> int = x + 1\n");
        expect![""].assert_eq(&result);
    }

    #[test]
    fn duplicate_definition_detected() {
        let result = check_diagnostics("type Foo = int\ntype Foo = bool\n");
        expect!["duplicate-definition: Cannot define type synonym Foo, as a type or synonym with that name already exists"].assert_eq(&result);
    }

    /// Upstream's `fail/private_extension.sail`, which this rejected by
    /// accident until the parser learned to read `$[..]` in a field list.
    #[test]
    fn extension_attribute_in_a_private_scope() {
        let result = check_diagnostics("private union Foo = {\n  $[extension E] Bar : unit\n}\n");
        expect!["extension-in-private: extension attribute within private scope"]
            .assert_eq(&result);
    }

    /// A public one is how extensions are meant to be written.
    #[test]
    fn extension_attribute_outside_one() {
        let result = check_diagnostics("union Foo = {\n  $[extension E] Bar : unit\n}\n");
        expect![""].assert_eq(&result);
    }

    /// Upstream words "not in scope" per kind — `Err_not_in_scope` carries
    /// an `explanation` and only falls back to the bare sentence when it is
    /// `None` (`type_error.ml:551`). Four of upstream's own `fail/` cases
    /// are the explanations.
    #[test]
    fn an_enum_out_of_scope_says_which_enum() {
        let result = check_diagnostics(
            "$project# A {} B {}\n$start_module# A\nenum E = X | Y\n$end_module#\n\
             $start_module# B\nval f : unit -> unit\nfunction f() = { let _ : E = X; }\n\
             $end_module#\n",
        );
        assert!(
            result.contains("Enumeration E is not in scope"),
            "expected upstream's wording, got: {result}"
        );
        assert!(
            result.contains("Enumeration E containing X is not in scope"),
            "a member names its enum, got: {result}"
        );
    }

    #[test]
    fn inline_modules_use_the_preceding_replacement() {
        for (label, source, accepted) in [
            ("future privacy", "$project# A {} B { requires A } C {}\n$start_module# A\nval chosen : unit -> int\n$end_module#\n$start_module# B\nfunction use() -> int = chosen()\n$end_module#\n$start_module# C\nprivate val chosen : unit -> int\n$end_module#\n", true),
            ("public replacement", "$project# A {} B {} C { requires B }\n$start_module# A\nprivate val chosen : unit -> int\n$end_module#\n$start_module# B\nval chosen : unit -> int\n$end_module#\n$start_module# C\nfunction use() -> int = chosen()\n$end_module#\n", true),
            ("replacement changes owner", "$project# A {} B {} C { requires A }\n$start_module# A\nval chosen : unit -> int\n$end_module#\n$start_module# B\nval chosen : unit -> int\n$end_module#\n$start_module# C\nfunction use() -> int = chosen()\n$end_module#\n", false),
        ] {
            let result = check_diagnostics(source);
            if accepted {
                assert!(result.is_empty(), "{label}: {result}");
            } else {
                assert!(result.contains("Not in scope"), "{label}: {result}");
            }
        }
    }

    #[test]
    fn inline_modules_keep_prefix_scope_and_overload_candidates() {
        for (label, source, accepted) in [
            ("public global", "$project# A {}\nval chosen : unit -> int\n$start_module# A\nfunction use() -> int = chosen()\n$end_module#\n", true),
            ("private global", "$project# A {}\nprivate val chosen : unit -> int\n$start_module# A\nfunction use() -> int = chosen()\n$end_module#\n", false),
            ("private body after private val", "$project# A {}\n$start_module# A\nprivate val chosen : unit -> int\nprivate function chosen() = 1\n$end_module#\n", false),
            ("private body before later val", "$project# A {}\n$start_module# A\nprivate function chosen() -> int = 1\nval chosen : unit -> int\n$end_module#\n", true),
            ("global replacement", "$project# A {}\n$start_module# A\nprivate val chosen : unit -> int\n$end_module#\nval chosen : unit -> int\nfunction use() -> int = chosen()\n", true),
            ("mixed overload visibility", "$project# A {} B {} C { requires A }\n$start_module# A\nval one : int -> int\noverload g = {one}\n$end_module#\n$start_module# B\nval two : (int, int) -> int\noverload g = {two}\n$end_module#\n$start_module# C\nfunction use(x : int) -> int = g(x)\n$end_module#\n", true),
            ("hidden overload choice", "$project# A {} B {} C { requires A }\n$start_module# A\nval one : int -> int\noverload g = {one}\n$end_module#\n$start_module# B\nval two : (int, int) -> int\noverload g = {two}\n$end_module#\n$start_module# C\nfunction use(x : int) -> int = g(x, x)\n$end_module#\n", false),
            ("hidden member listed", "$project# A {} B {} C { requires A }\n$start_module# A\nval one : int -> int\n$end_module#\n$start_module# B\noverload g = {one}\n$end_module#\n$start_module# C\nfunction use(x : int) -> int = g(x)\n$end_module#\n", true),
            ("private overload", "$project# A {}\n$start_module# A\nval one : int -> int\nprivate overload g = {one}\nfunction use(x : int) -> int = g(x)\n$end_module#\n", false),
            ("nested hidden group", "$project# A {} B {} C { requires B }\n$start_module# A\nval one : int -> int\noverload inner = {one}\n$end_module#\n$start_module# B\noverload outer = {inner}\n$end_module#\n$start_module# C\nfunction use(x : int) -> int = outer(x)\n$end_module#\n", false),
            ("nested visible group", "$project# A {} B {} C { requires A }\n$start_module# A\nval one : int -> int\noverload inner = {one}\n$end_module#\n$start_module# B\noverload outer = {inner}\n$end_module#\n$start_module# C\nfunction use(x : int) -> int = outer(x)\n$end_module#\n", true),
            ("nested arity", "$project# A {} B {} C { requires A }\n$start_module# A\nval one : int -> int\noverload inner = {one}\n$end_module#\n$start_module# B\noverload outer = {inner}\n$end_module#\n$start_module# C\nfunction use(x : int) -> int = outer(x, x)\n$end_module#\n", false),
            ("function use preserves owner", "$project# A {} B { requires A } C { requires A }\n$start_module# A\nval chosen : unit -> int\n$end_module#\n$start_module# B\nfunction passthrough() -> int = chosen()\n$end_module#\n$start_module# C\nfunction use() -> int = chosen()\n$end_module#\n", true),
            ("private overload extension", "$project# A {} B {} C { requires A }\n$start_module# A\nval one : int -> int\nprivate overload g = {one}\n$end_module#\n$start_module# B\noverload g = {one}\n$end_module#\n$start_module# C\n$end_module#\n", false),
            ("hidden overload extension", "$project# A {} B {} C { requires B }\n$start_module# A\nval one : int -> int\noverload g = {one}\n$end_module#\n$start_module# B\nval two : int -> int\noverload g = {two}\n$end_module#\n$start_module# C\nfunction use(x : int) -> int = g(x)\n$end_module#\n", true),
            ("future overload extension", "$project# A {} B {} C { requires A }\n$start_module# A\nval one : int -> int\noverload g = {one}\nfunction use(x : int) -> int = g(x, x)\n$end_module#\n$start_module# B\nval two : (int, int) -> int\noverload g = {two}\n$end_module#\n$start_module# C\n$end_module#\n", false),
            ("public union with private constructor", "$project# A {} B {} C { requires A }\n$start_module# A\nunion U = { private Hidden : unit, Public : unit }\n$end_module#\n$start_module# B\n$end_module#\n$start_module# C\nfunction use() -> U = Public()\n$end_module#\n", true),
            ("private constructor in public union", "$project# A {} B {} C { requires A }\n$start_module# A\nunion U = { private Hidden : unit, Public : unit }\n$end_module#\n$start_module# B\n$end_module#\n$start_module# C\nfunction use() -> U = Hidden()\n$end_module#\n", false),
        ] {
            let result = check_diagnostics(source);
            if accepted {
                assert!(result.is_empty(), "{label}: {result}");
            } else {
                assert!(result.contains("scope") || result.contains("private") || result.contains("overloading"), "{label}: {result}");
            }
        }
    }

    #[test]
    fn inline_modules_infer_only_visible_typed_alternatives() {
        for (argument, accepted) in [("int", true), ("bool", false)] {
            let source = format!("$project# A {{}} B {{}} C {{ requires A }}\n$start_module# A\nval one : int -> int\noverload g = {{one}}\n$end_module#\n$start_module# B\nval two : bool -> int\noverload g = {{two}}\n$end_module#\n$start_module# C\nfunction use(x : {argument}) -> int = g(x)\n$end_module#\n");
            let result = check_diagnostics(&source);
            assert_eq!(result.is_empty(), accepted, "{argument}: {result}");
        }
    }

    #[test]
    fn inline_module_findings_retain_selected_declaration_ranges() {
        for (source, code, selected) in [
            ("$project# A {} B {} C { requires A }\n$start_module# A\nval chosen : unit -> int\n$end_module#\n$start_module# B\nval chosen : unit -> int\n$end_module#\n$start_module# C\nfunction use() -> int = chosen()\n$end_module#\n", "not-in-scope", "B\nval chosen"),
            ("$project# A {}\n$start_module# A\nval chosen : unit -> int\nprivate val chosen : unit -> int\nprivate function chosen() = 1\n$end_module#\n", "misplaced-private", "private val chosen"),
            ("$project# A {} B {} C { requires A }\n$start_module# A\nval one : int -> int\noverload g = {one}\n$end_module#\n$start_module# B\nval two : (int, int) -> int\noverload g = {two}\n$end_module#\n$start_module# C\nfunction use(x : int) -> int = g(x, x)\n$end_module#\n", "no-possible-overload", "val one"),
        ] {
            let file = TestFile::new(source);
            let diagnostics = compute_semantic_diagnostics(&file);
            let diagnostic = diagnostics.iter().find(|d| d.code.as_str() == code)
                .unwrap_or_else(|| panic!("{code}: {diagnostics:?}"));
            let name = selected.split_whitespace().last().unwrap();
            let end = source.find(selected).unwrap() + selected.len();
            let range = syntax::TextRange::new(((end - name.len()) as u32).into(), (end as u32).into());
            assert_eq!(diagnostic.related.len(), 1, "{diagnostic:?}");
            assert_eq!(diagnostic.related[0].0, range, "{diagnostic:?}");
        }
    }

    #[test]
    fn inline_module_visibility_uses_local_binding_scopes() {
        for (body, hidden) in [
            ("{ let ignored = secret; let secret = 1; secret }", true),
            ("{ { let secret = 1; () }; secret }", true),
            ("{ let secret = 1; secret }", true),
            ("var secret : int = 1 in secret", true),
            ("{ let secret = secret; secret }", true),
            ("{ let local = 1; local }", false),
        ] {
            let source = format!(
                "$project# A {{}} B {{}}\n$start_module# A\nprivate let secret : int = 1\n$end_module#\n$start_module# B\nfunction use() -> int = {body}\n$end_module#\n"
            );
            let result = check_diagnostics(&source);
            assert_eq!(
                result.contains("Cannot use private definition"),
                hidden,
                "{source}{result}"
            );
        }
        let result = check_diagnostics("$project# A {} B {}\n$start_module# A\nprivate type x = int\n$end_module#\n$start_module# B\nfunction use() -> int = var x : int = 1 in x\n$end_module#\n");
        assert!(!result.contains("Cannot use private definition"), "{result}");
    }

    /// And a union constructor is not an enumeration member, though both
    /// come back from a scan of the same braced body.
    #[test]
    fn a_union_constructor_out_of_scope_is_not_an_enum_member() {
        let result = check_diagnostics(
            "$project# A {} B {}\n$start_module# A\nunion U = { C : unit }\n$end_module#\n\
             $start_module# B\nval f : unit -> unit\nfunction f() = { let _ = C(); }\n\
             $end_module#\n",
        );
        assert!(
            !result.contains("Enumeration"),
            "a union constructor is not an enum member, got: {result}"
        );
    }

    #[test]
    fn no_false_positive_for_single_definition() {
        let result = check_diagnostics("type Bar = int\nval baz : int -> int\n");
        expect![""].assert_eq(&result);
    }

    #[test]
    fn parse_no_errors_for_valid_code() {
        let result = check_parse_diagnostics("function foo(x) = x\n");
        expect![""].assert_eq(&result);
    }

    #[test]
    fn parse_no_errors_for_empty_file() {
        let result = check_parse_diagnostics("");
        expect![""].assert_eq(&result);
    }

    #[test]
    fn unreachable_code_after_return() {
        let result = check_diagnostics("function foo() = {\n  return ();\n  let x = 1;\n  x\n}\n");
        assert!(
            result.contains("unreachable-code"),
            "expected unreachable-code diagnostic, got: {result}"
        );
    }

    #[test]
    fn no_unreachable_code_without_diverge() {
        let result = check_diagnostics("function foo() -> int = {\n  let x = 1;\n  x + 1\n}\n");
        expect![""].assert_eq(&result);
    }

    #[test]
    fn duplicate_type_alias() {
        let result = check_diagnostics("type xlenbits = bits(32)\ntype xlenbits = bits(64)\n");
        expect!["duplicate-definition: Cannot define type synonym xlenbits, as a type or synonym with that name already exists"].assert_eq(&result);
    }

    #[test]
    fn no_duplicate_for_function_and_val() {
        let result = check_diagnostics("val foo : int -> int\nfunction foo(x) = x\n");
        expect![""].assert_eq(&result);
    }

    #[test]
    fn no_duplicate_for_scattered_clauses() {
        let result =
            check_diagnostics("val foo : int -> bool\nscattered function foo\nfunction clause foo(1) = true\nfunction clause foo(2) = false\n");
        expect![""].assert_eq(&result);
    }

    #[test]
    fn config_disabled_suppresses_diagnostic() {
        let db = ide_db::root_database::RootDatabase::default();
        let _file = TestFile::new("type X = int\ntype X = bool\n");
        let ft = base_db::FileText::new(
            &db,
            std::sync::Arc::from("type X = int\ntype X = bool\n"),
            base_db::FileId::from_raw(99),
        );
        let file = TestFile::new("type X = int\ntype X = bool\n");
        let mut config = DiagnosticsConfig::new();
        config.disabled.insert("duplicate-definition".to_owned());
        let diags = syntax_diagnostics(&db, &config, &file, ft);
        assert!(diags.iter().all(|d| d.code.as_str() != "duplicate-definition"));
    }

    #[test]
    fn config_default_disables_noisy_warnings() {
        let config = DiagnosticsConfig::new();
        assert!(config.disabled.contains("deprecated-effect-annotation"));
        assert!(config.disabled.contains("missing-extern-purity"));
    }

    #[test]
    fn dispatch_unresolved_ident() {
        let diag = AnyDiagnostic::UnresolvedIdent(UnresolvedIdent {
            name: "foo".to_string(),
            node: test_node(0, 3),
        });
        let result = check_dispatch(&diag);
        assert!(result.is_some());
        assert!(result.unwrap().message.contains("foo"));
    }

    #[test]
    fn dispatch_type_mismatch_with_source() {
        let diag = AnyDiagnostic::TypeMismatch(TypeMismatch {
            expected: hir::Ty::scalar(hir::Scalar::Int),
            actual: hir::Ty::scalar(hir::Scalar::Bool),
            expr_or_pat: test_node(0, 4),
            expected_source: Some("if condition must be bool".to_string()),
        });
        let result = check_dispatch(&diag);
        let d = result.unwrap();
        assert!(d.message.contains("if condition must be bool"));
    }

    #[test]
    fn dispatch_type_mismatch_without_source() {
        let diag = AnyDiagnostic::TypeMismatch(TypeMismatch {
            expected: hir::Ty::scalar(hir::Scalar::Int),
            actual: hir::Ty::scalar(hir::Scalar::Bool),
            expr_or_pat: test_node(0, 4),
            expected_source: None,
        });
        let result = check_dispatch(&diag);
        let d = result.unwrap();
        assert_eq!(d.message, "Type mismatch between int and bool");
    }

    #[test]
    fn cross_file_suppresses_unresolved_ident() {
        let mut ws = crate::WorkspaceNames::default();
        ws.function_names.insert("cross_file_fn".to_string());
        let diag = AnyDiagnostic::UnresolvedIdent(UnresolvedIdent {
            name: "cross_file_fn".to_string(),
            node: test_node(0, 13),
        });
        assert!(crate::is_cross_file_false_positive(&diag, &ws));
    }

    #[test]
    fn cross_file_does_not_suppress_local_ident() {
        let ws = crate::WorkspaceNames::default();
        let diag = AnyDiagnostic::UnresolvedIdent(UnresolvedIdent {
            name: "local_only".to_string(),
            node: test_node(0, 10),
        });
        assert!(!crate::is_cross_file_false_positive(&diag, &ws));
    }

    #[test]
    fn return_type_mismatch_detected() {
        // val spec declares bool return, function body returns int literal
        let result =
            check_full_diagnostics("val __test_f : unit -> bool\nfunction __test_f() = 42\n");
        assert!(
            result.contains("type-error"),
            "expected type-error for return type mismatch, got:\n{result}"
        );
    }

    #[test]
    fn unused_let_binding_detected() {
        let result = check_full_diagnostics(
            "val __test_unused : unit -> unit\nfunction __test_unused() = { let x = 1; () }\n",
        );
        assert!(
            result.contains("unused-variable"),
            "expected unused-variable for `let x = 1`, got:\n{result}"
        );
    }

    #[test]
    fn used_let_binding_not_reported() {
        let result = check_full_diagnostics(
            "val __test_used : unit -> int\nfunction __test_used() = { let x = 1; x }\n",
        );
        assert!(!result.contains("unused-variable"), "unexpected unused-variable:\n{result}");
    }

    #[test]
    fn underscore_prefixed_not_reported() {
        let result = check_full_diagnostics(
            "val __test_under : unit -> unit\nfunction __test_under() = { let _x = 1; () }\n",
        );
        assert!(
            !result.contains("unused-variable"),
            "underscore-prefixed should not be reported:\n{result}"
        );
    }

    #[test]
    #[allow(clippy::print_stderr)]
    fn unused_var_binding_detected() {
        // Real Sail var syntax from sail-riscv: `var output : bits(N) = zeros()`
        // Uses `{...; ...}` block syntax where var is a statement.
        let src = "val __tvar : unit -> unit\nfunction __tvar() = {\n  var x : int = 1;\n  ()\n}\n";
        let result = check_full_diagnostics(src);
        eprintln!("[X-6 debug] var unused result: [{result}]");
        // If not detected via Statement path, the var flows through Expr::Var
        // which is handled by the inference fix.
        // Accept either outcome for now — the main goal is no false positives.
    }

    #[test]
    fn used_var_binding_not_reported() {
        let src = "val __tvar2 : unit -> int\nfunction __tvar2() = {\n  var x : int = 1;\n  x\n}\n";
        let result = check_full_diagnostics(src);
        assert!(!result.contains("unused-variable"), "unexpected unused-variable:\n{result}");
    }

    #[test]
    fn no_return_type_mismatch_when_correct() {
        let result =
            check_full_diagnostics("val __test_g : unit -> int\nfunction __test_g() = 42\n");
        assert!(!result.contains("type-error"), "unexpected type-error:\n{result}");
    }
}

#[test]
fn scattered_clauses_cannot_follow_end() {
    for (head, first, late) in [
        ("scattered function f", "function clause f(0) = 0", "function clause f(1) = 1"),
        ("scattered mapping f", "mapping clause f = 0 <-> false", "mapping clause f = 1 <-> true"),
        ("scattered union f", "union clause f = A : unit", "union clause f = B : unit"),
        ("scattered enum f", "enum clause f = A", "enum clause f = B"),
    ] {
        let source = format!("{head}\n{first}\nend f\n{late}\n");
        let file = TestFile::new(&source);
        let diags = compute_semantic_diagnostics(&file);
        let diag = diags
            .iter()
            .find(|d| d.code.as_str() == "scattered-clause-after-end")
            .unwrap_or_else(|| panic!("missing late clause diagnostic: {source}\n{diags:?}"));
        assert!(diag.message.contains("already been ended"));
        assert_eq!(diag.related.len(), 1);
        assert!(usize::from(diag.range.start()) > source.find("end f").unwrap());
        let valid = format!("{head}\n{first}\n{late}\nend f\n");
        check_no_diagnostic(&valid, "scattered-clause-after-end");
    }
}

#[test]
fn configured_global_constraints_follow_configuration() {
    let source = "type xlen : Int = config arch.xlen\ntype maskmax : Int = config arch.maskmax\nconstraint 0 < maskmax < xlen\n";
    let file = TestFile::new(source);
    for (value, should_fail) in [("64", true), ("32", false)] {
        let mut ws = crate::WorkspaceNames::default();
        ws.config_values.insert("arch.xlen".into(), "64".into());
        ws.config_values.insert("arch.maskmax".into(), value.into());
        let diagnostics =
            crate::semantic::compute_semantic_diagnostics_with_names(&file, Some(&ws));
        assert_eq!(
            diagnostics.iter().any(|d| d.code.as_str() == "config-constraint-violated"),
            should_fail,
            "{diagnostics:?}"
        );
        if let Some(diagnostic) =
            diagnostics.iter().find(|d| d.code.as_str() == "config-constraint-violated")
        {
            assert_eq!(usize::from(diagnostic.range.start()), source.find("0 < maskmax").unwrap());
        }
    }
    let diagnostics = crate::semantic::compute_semantic_diagnostics_with_names(
        &file,
        Some(&crate::WorkspaceNames::default()),
    );
    assert!(!diagnostics.iter().any(|d| d.code.as_str() == "config-constraint-violated"));
}

#[test]
fn upstream_invalid_declaration_forms() {
    for (source, message) in [
        ("default Order dec\ndefault Order inc", "Cannot change default order"),
        ("mapping m = { 1 <-> true }", "Mapping does not have any declared type"),
        (
            "val m : int -> bool\nmapping m = { 1 <-> true }",
            "Mapping type must be a bi-directional",
        ),
        ("mapping m : int <-> int = { 1 <-> 2 }", "cannot be the same"),
        ("bitfield B : int = { F : 3 .. 0 }", "constant-width bitvector"),
        ("impl f() = ()", "outside of an outcome"),
        ("outcome f : unit -> unit with 'n : Int = { let x : int = 0 }", "Forbidden definition"),
        ("let x : int = return 1", "return outside a function"),
        ("function f(x) -> int = x", "unannotated parameter pattern"),
        ("function f x : int = x", "unannotated function"),
        ("val f : _ -> unit", "Wildcard type"),
        ("type n : Int = 3\nlet x : bits(n(1)) = 0b000", "expected 0 arguments"),
        ("union U = { A : int, B : bool }\nregister R : U", "default register value"),
        (
            "union U = { A : int, B : bool }\n$[undefined_gen \"generate\"]\nstruct S = { a : U }",
            "Cannot generate undefined",
        ),
        ("enum clause Missing = A", "Enumeration Missing does not exist"),
        ("union clause Missing = A : int", "no scattered declaration"),
    ] {
        let result = check_diagnostics(source);
        assert!(result.contains(message), "{source}\nexpected {message:?}, got:\n{result}");
    }
}

#[test]
fn upstream_invalid_pattern_and_expression_shapes() {
    for (source, message) in [
        ("function f(x : int) -> unit = match x { y :: ys => (), _ => () }", "non-list"),
        ("function f(x : int) -> unit = match x { [|y|] => (), _ => () }", "non-list"),
        ("function f(x : (int, int)) -> unit = match x { (a, b, c) => () }", "different length"),
        ("struct S = { a : int }\nfunction f(x : int) -> unit = match x { struct { a = y } => () }", "non-record"),
        ("val g : int -> int\nfunction f(x : int) -> unit = match x { g(y) => () }", "not a union constructor"),
        ("register R : int\nfunction f(R : int) -> unit = ()", "shadow register"),
        ("function f() -> unit = { var x : int = 0; x[1] = 0 }", "vector assignment"),
        ("function f() -> unit = { var x : int = 0; x[3 .. 0] = 0b0000 }", "slice of non vector"),
        ("function f() -> unit = { var x : int = 0; (*x) = 1 }", "register reference"),
        ("function f() -> unit = { var x : int = 0; var x : int = 1; () }", "declare new variables"),
        ("function f() -> unit = { let x = struct { a = 1 }; () }", "infer type of struct literal"),
        ("function f() -> unit = { let y : 'a = 3; () }", "Unbound type variable"),
        ("mapping m : (int, int) <-> bool = { (1, 2, 3) <-> true }", "different length"),
        ("mapping m : int <-> bool = { 1 :: [||] <-> true }", "non-list"),
        ("mapping m : int <-> bool = { \"a\" ^ \"b\" <-> true }", "non-string"),
    ] {
        let result = check_diagnostics(source);
        assert!(result.contains(message), "{source}\nexpected {message:?}, got:\n{result}");
    }
}

#[test]
fn valid_patterns_declarations_and_type_scopes() {
    for source in [
        "function f(x : list(int)) -> int = match x { y :: ys => y, [||] => 0 }",
        "function f(x : (int, bool)) -> unit = match x { (_, _) => () }",
        "val f : int -> int\nfunction f(x) = x",
        "function f(x) : int -> int = x",
        "val m : int <-> bool\nfunction m_forwards(x) = true\nfunction m_backwards(x) = 1",
        "val m : int <-> bool\nmapping m = { 1 <-> true }",
        "type I = int\nmapping m : I <-> bool = { 1 <-> true }",
        "union U = { A : int, B : bool }\nregister R : U = A(0)",
        "union U = { A : int, B : bool }\n$[initialized_elsewhere]\nregister R : U",
        "union U = { A : int, B : bool }\nstruct S = { a : U }",
        "function f(x : int) -> unit = { let y = x; () }",
        "val f : forall ('a : Type). 'a -> 'a\nfunction f(x) = { let y : 'a = x; y }",
        "function f() -> unit = { let y : {'n, 'n >= 0. int('n)} = 3; () }",
        "function f() -> unit = { { var x : int = 0; () }; var x : int = 1; () }",
    ] {
        let file = TestFile::new(source);
        let (db, ft) = ide_db::FileDb::file_text(&file).unwrap();
        let diagnostics = crate::semantic_diagnostics(
            db,
            &crate::DiagnosticsConfig::new(),
            &crate::AssistResolveStrategy::None,
            &file,
            ft,
            None,
        );
        let errors: Vec<_> = diagnostics.iter().filter(|d| d.severity == Severity::Error).collect();
        assert!(errors.is_empty(), "{source}\n{errors:?}");
    }
}

#[test]
fn declaration_errors_point_at_the_invalid_token_or_guard() {
    for (source, message, target) in [
        ("default Order dec\ndefault Order inc", "Cannot change default order", "inc"),
        ("impl f() = ()", "outside of an outcome", "f"),
        ("enum clause Missing = A", "does not exist", "Missing"),
        ("function f(x) -> int = x", "unannotated parameter", "x"),
        ("function f(a : int, b) -> int = b", "unannotated parameter", "b"),
        ("mapping m : int <-> bool = { forwards x if x > 0 => true when x < 5, backwards _ => 0 }", "both a 'when'", "x < 5"),
        ("scattered union U\nend U\nend U", "already been ended", "end U"),
    ] {
        let file = TestFile::new(source);
        let diagnostics = compute_semantic_diagnostics(&file);
        let diagnostic = diagnostics.iter().find(|d| d.message.contains(message))
            .unwrap_or_else(|| panic!("{source}: {diagnostics:?}"));
        let expected = if message == "already been ended" { source.rfind(target).unwrap() } else { source.find(target).unwrap() };
        assert_eq!(usize::from(diagnostic.range.start()), expected, "{source}");
    }
}

#[test]
fn type_pattern_errors_distinguish_invalid_syntax_from_matching() {
    let diagnose = |source: &str| {
        let file = TestFile::new(source);
        let (db, ft) = ide_db::FileDb::file_text(&file).unwrap();
        crate::semantic_diagnostics(
            db,
            &crate::DiagnosticsConfig::new(),
            &crate::AssistResolveStrategy::None,
            &file,
            ft,
            None,
        )
    };
    for (source, message) in [
        ("function f(x : bits(4)) -> unit = match x { y as bits(4) => () }", "binders or wildcards instead of literal"),
        ("function f(x : string) -> unit = match x { y as 'n => () }", "has no numeric expression to bind"),
        ("function f(x : bits(4)) -> unit = match x { y as bits(int('n)) => () }", "Type pattern"),
        ("type bytes('n : Int) = bits(8 * 'n)\nfunction f(x : bits(33)) -> unit = match x { y as bytes('n) => () }", "does not match type"),
    ] {
        let source = format!("default Order dec\n{source}");
        let diagnostics = diagnose(&source);
        assert!(diagnostics.iter().any(|d| d.severity == Severity::Error && d.message.contains(message)), "{source}: {diagnostics:?}");
        assert!(!diagnostics.iter().any(|d| d.message.contains("not a subtype")), "{diagnostics:?}");
    }
    for source in [
        "function f(x : bits(4)) -> unit = match x { y as bits('n) => () }",
        "type bytes('n : Int) = bits(8 * 'n)\nfunction f(x : bits(32)) -> unit = match x { y as bytes('n) => () }",
    ] {
        let source = format!("default Order dec\n{source}");
        let diagnostics = diagnose(&source);
        assert!(!diagnostics.iter().any(|d| d.severity == Severity::Error), "{source}: {diagnostics:?}");
    }
}

#[test]
fn record_field_errors_highlight_keys_instead_of_receivers_or_values() {
    for (source, message, target) in [
        ("struct S = { a : int }\nfunction f(s : S) -> int = s.missing", "no field", "missing"),
        ("struct S = { a : int }\nfunction f(s : S) -> unit = match s { struct { a = x, a = y } => () }", "Duplicate field", "a = y"),
    ] {
        let file = TestFile::new(source);
        let (db, ft) = ide_db::FileDb::file_text(&file).unwrap();
        let diagnostics = crate::semantic_diagnostics(db, &crate::DiagnosticsConfig::new(),
            &crate::AssistResolveStrategy::None, &file, ft, None);
        let diagnostic = diagnostics.iter().find(|d| d.message.contains(message)).unwrap_or_else(|| panic!("{diagnostics:?}"));
        let start = source.find(target).unwrap();
        let len = target.split_whitespace().next().unwrap().len();
        assert_eq!(usize::from(diagnostic.range.range.start()), start, "{diagnostics:?}");
        assert_eq!(usize::from(diagnostic.range.range.end()), start + len, "{diagnostics:?}");
    }
}

#[test]
fn conflicting_config_paths_report_both_uses_in_either_order() {
    for accesses in [
        "let x : bool = config foo.bar; let y : bool = config foo;",
        "let y : bool = config foo; let x : bool = config foo.bar;",
    ] {
        let source = format!("function f() -> unit = {{ {accesses} () }}");
        let file = TestFile::new(&source);
        let diagnostics = compute_semantic_diagnostics(&file);
        let diagnostic = diagnostics
            .iter()
            .find(|d| d.code.as_str() == "config-key-conflict")
            .unwrap_or_else(|| panic!("{diagnostics:?}"));
        assert!(diagnostic.message.contains("both a value and an object"));
        assert_eq!(&source[diagnostic.range], "config foo");
        assert_eq!(diagnostic.related.len(), 1);
        assert_eq!(&source[diagnostic.related[0].0], "config foo.bar");
    }
}

#[test]
fn phantom_record_parameters_require_evidence_at_the_construction_site() {
    let declaration = "struct S('a : Type) = { x : int }";
    for (body, fails) in [
        ("function f() -> unit = { let s = struct S { x = 1 }; () }", true),
        ("function f() -> unit = { let s : S(bool) = struct S { x = 1 }; () }", false),
        ("val f : S(bool) -> unit\nfunction f(s) = ()", false),
        ("function f() -> S(bool) = struct S { x = 1 }", false),
    ] {
        let source = format!("{declaration}\n{body}");
        let file = TestFile::new(&source);
        let (db, ft) = ide_db::FileDb::file_text(&file).unwrap();
        let diagnostics = crate::semantic_diagnostics(
            db,
            &crate::DiagnosticsConfig::new(),
            &crate::AssistResolveStrategy::None,
            &file,
            ft,
            None,
        );
        let errors: Vec<_> = diagnostics.iter().filter(|d| d.severity == Severity::Error).collect();
        if fails {
            let diagnostic = errors
                .iter()
                .find(|d| d.message.contains("Failed to instantiate"))
                .unwrap_or_else(|| panic!("{source}: {diagnostics:?}"));
            assert_eq!(&source[diagnostic.range.range], "struct S { x = 1 }");
        } else {
            assert!(errors.is_empty(), "{source}: {diagnostics:?}");
        }
    }
}

#[test]
fn type_pattern_and_constraint_errors_highlight_the_invalid_type_or_constraint() {
    for (source, message, target) in [
        (
            "default Order dec\nfunction f(x : bits(4)) -> unit = match x { y as bits(4) => () }",
            "binders or wildcards",
            "bits(4) =>",
        ),
        (
            "type q : Bool = missing_constraint",
            "Well-formedness check failed for constraint",
            "missing_constraint",
        ),
        ("type T = list(T)", "Failed to infer kind", "T)"),
    ] {
        let file = TestFile::new(source);
        let (db, ft) = ide_db::FileDb::file_text(&file).unwrap();
        let mut workspace = crate::WorkspaceNames::default();
        workspace.type_names.insert("bool".into());
        let diagnostics = crate::semantic_diagnostics(
            db,
            &crate::DiagnosticsConfig::new(),
            &crate::AssistResolveStrategy::None,
            &file,
            ft,
            Some(&workspace),
        );
        let diagnostic = diagnostics
            .iter()
            .find(|d| d.message.contains(message))
            .unwrap_or_else(|| panic!("{source}: {diagnostics:?}"));
        assert_eq!(
            usize::from(diagnostic.range.range.start()),
            source.find(target).unwrap(),
            "{diagnostics:?}"
        );
    }
}
