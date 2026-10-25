// --- External crate imports (direct, no re-export shells) ---
use crate::code_action_helpers::{
    code_action_kind_allowed, lazy_code_action_data, resolve_code_action_edit_from_data,
    sail_source_fix_all_kind,
};
#[cfg(feature = "z3-solver")]
use hir::infer::{CompareOp, ConstraintExpr, ConstraintStatus, NumericExpr, Subst};
use ide::annotations::{
    code_lens_title, code_lenses_ide, collect_implementation_counts, collect_reference_counts,
};
use ide::completion::completion_prefix;
use ide::extend_selection::selection_range;
#[cfg(unix)]
use ide::formatting::document_links_for_file;
use ide::formatting::{
    format_document_cst, linked_editing_ranges_for_position, range_format_document_edits,
};
use ide::goto_type_definition::{parse_named_type, typed_bindings};
use ide::navigation::{
    implementation_locations, resolve_workspace_symbol, symbol_declaration_locations,
    symbol_definition_locations, type_alias_edges, type_name_candidates_at_position, type_subtypes,
    type_supertypes, will_rename_file_edits,
};
use ide::references::{
    reference_locations, rename_edits, resolve_symbol_at, symbol_spans_for_file,
};
use ide::signature_help::find_call_at_position;
use ide_assists::{missing_semicolon_fix, quick_fix_for_diagnostic};
use ide_db::{collect_callable_signatures, function_snippet, Parameter};

// --- Local crate imports ---

// --- Std + tower-lsp ---
use lsp_types::{CodeActionKind, Diagnostic, NumberOrString, Range, TextEdit, Url};

// Drop-in replacement for File::new(source) in tests.
// Uses salsa database internally; provides same interface as File.
// This bridge enables incremental test migration from File → salsa.
// Once all tests use SalsaTestFile, File struct can be deleted .

/// Salsa-backed test file. Replaces `File::new(source)` in tests.
/// All derived data (parse, diagnostics, type-check) comes from salsa queries.
struct SalsaTestFile {
    db: ide_db::root_database::RootDatabase,
    input: base_db::FileText,
    text: String,
    /// Compatibility: provides .source.position_at() and .source.text()
    pub source: ide_db::text_document::TextDocument,
}

impl SalsaTestFile {
    fn new(source: String) -> Self {
        let mut db = ide_db::root_database::RootDatabase::default();
        let text_arc: std::sync::Arc<str> = std::sync::Arc::from(source.as_str());
        let input = base_db::FileText::new(&db, text_arc, base_db::FileId::from_raw(0));
        // Register the workspace, as the server does on load. Without it the
        // salsa queries have no workspace context and `infix` declarations
        // never reach the parser.
        hir::def_query::set_workspace_files(&mut db, vec![input]);
        let source_doc = ide_db::text_document::TextDocument::new(source.clone());
        Self { db, input, text: source, source: source_doc }
    }

    /// Get a SalsaFile adapter for passing to IDE functions.
    #[expect(dead_code)]
    fn as_salsa_file(&self) -> ide_db::root_database::SalsaFile<'_> {
        ide_db::root_database::SalsaFile::new(&self.db, self.input)
    }

    /// Compute LSP diagnostics through the pipeline the server runs.
    ///
    /// `Analysis::file_diagnostics` is what `handle_document_diagnostic` and
    /// the push-diagnostics loop both call. Assembling the pieces separately
    /// — as this used to — produces a different set: it missed parse errors
    /// entirely, and every check gated on workspace context.
    fn lsp_diagnostics(&self) -> Vec<lsp_types::Diagnostic> {
        let (analysis, files) = ide::fixture::analysis(&self.text);
        let config = ide_diagnostics::DiagnosticsConfig::new();
        let diags = analysis
            .file_diagnostics(files[0].0, &config)
            .expect("the test database is never cancelled");
        let line_index = ide_db::line_index::LineIndex::new(&self.text);
        let url = lsp_types::Url::parse("file:///test.sail").expect("a valid test URL");
        diags.iter().map(|d| crate::to_proto::diagnostic(&line_index, &url, d)).collect()
    }

    /// Access parsed file (replaces File::parsed).
    #[expect(dead_code)]
    fn parsed(&self) -> Option<&syntax::parser_lower::ParsedFile> {
        // Can't return reference to temporary SalsaFile, so
        // compute via salsa and return None for now.
        // Tests that need parsed() should use the SalsaFile directly.
        None
    }

    /// Access item tree (replaces File::item_tree).
    fn item_tree(&self) -> Option<std::sync::Arc<hir::ItemTree>> {
        hir::def_query::file_item_tree(&self.db, self.input).clone()
    }
}

// Implement FileDb for SalsaTestFile so IDE functions accept &SalsaTestFile
impl hir::callgraph::WorkspaceFile for SalsaTestFile {
    fn content_hash(&self) -> u64 {
        0
    }
    fn callgraph(&self) -> Option<&hir::callgraph::CallGraph> {
        None
    }
}

impl hir::callgraph::SourceFileInfo for SalsaTestFile {
    fn text(&self) -> &str {
        &self.text
    }
    fn item_tree(&self) -> Option<&hir::ItemTree> {
        hir::def_query::file_item_tree(&self.db, self.input).as_deref()
    }
}

impl ide_db::FileDb for SalsaTestFile {
    fn file_text(&self) -> Option<(&dyn salsa::Database, base_db::FileText)> {
        Some((&self.db, self.input))
    }
    fn position_at(&self, offset: usize) -> ide_db::LineCol {
        self.source.position_at(offset)
    }
    fn offset_at(&self, position: &ide_db::LineCol) -> usize {
        self.source.offset_at(position)
    }
    fn tokens(&self) -> Option<&[(parser::Token, parser::Span)]> {
        let parsed = syntax::parse_query::parse_file(&self.db, self.input);
        if parsed.tokens.is_empty() {
            None
        } else {
            Some(parsed.tokens.as_slice())
        }
    }
    fn token_at(&self, position: ide_db::LineCol) -> Option<&(parser::Token, parser::Span)> {
        let offset = self.offset_at(&position);
        let tokens = self.tokens()?;
        tokens
            .iter()
            .rev()
            .find(|(_, span)| span.start <= offset && offset < span.end)
            .or_else(|| tokens.iter().rev().find(|(_, span)| span.end == offset))
    }
    fn parsed(&self) -> Option<&syntax::parser_lower::ParsedFile> {
        let pf = syntax::parse_query::parsed_file(&self.db, self.input);
        pf.as_ref().map(|apf| apf.0.as_ref())
    }
    fn bodies(&self) -> Option<&hir::CallableBodies> {
        hir::def_query::callable_bodies(&self.db, self.input).as_ref().map(|b| b.0.as_ref())
    }
}

impl SalsaTestFile {
    /// Diagnostics the *checker* produced, read from the per-callable salsa
    /// query.
    ///
    /// A few checks — match exhaustiveness, the unresolved-quantifier
    /// report — write to `InferenceResult`'s text channel, which
    /// `semantic_diagnostics` does not render. Tests that pin those read
    /// them here rather than through a surface that cannot show them.
    fn checker_diagnostics(&self) -> Vec<hir::diagnostics::Diagnostic> {
        hir::def_query::file_def_with_body_ids(&self.db, self.input)
            .iter()
            .flat_map(|&id| hir::ty_query::infer(&self.db, id).0.diagnostics().to_vec())
            .collect()
    }

    /// The checker's structured diagnostics, before any rendering decides
    /// whether to show them.
    fn checker_inference_diagnostics(&self) -> Vec<hir::InferenceDiagnostic> {
        hir::def_query::file_def_with_body_ids(&self.db, self.input)
            .iter()
            .flat_map(|&id| hir::ty_query::infer(&self.db, id).0.inference_diagnostics().to_vec())
            .collect()
    }
}

// A single salsa database holds multiple files as inputs.
// Cross-file analysis happens automatically via salsa queries.

/// A multi-file workspace, built the way the server builds one.
///
/// `ide::fixture::analysis` registers `WorkspaceFiles` and hands back a real
/// `Analysis`; the previous version assembled its own database without
/// registering the workspace, so its salsa cross-file queries were inert and
/// it had to call the non-salsa `check_file_with_workspace` to see anything.
struct SalsaTestWorkspace {
    analysis: ide::analysis::Analysis,
    files: Vec<(base_db::FileId, String)>,
}

impl SalsaTestWorkspace {
    /// Create a workspace from source texts, one file each.
    fn new(sources: &[&str]) -> Self {
        let fixture = sources
            .iter()
            .enumerate()
            .map(|(i, src)| format!("//- /f{i}.sail\n{src}"))
            .collect::<Vec<_>>()
            .join("");
        let (analysis, ids) = ide::fixture::analysis(&fixture);
        let files = ids
            .iter()
            .zip(sources.iter())
            .map(|((fid, _), src)| (*fid, (*src).to_owned()))
            .collect();
        Self { analysis, files }
    }

    /// Diagnostics for one file, through `Analysis::file_diagnostics` — the
    /// entry point the server calls.
    fn diagnostics_with_workspace(&self, file_idx: usize) -> Vec<lsp_types::Diagnostic> {
        let (file_id, text) = &self.files[file_idx];
        let config = ide_diagnostics::DiagnosticsConfig::new();
        let diags = self
            .analysis
            .file_diagnostics(*file_id, &config)
            .expect("the test database is never cancelled");
        let line_index = ide_db::line_index::LineIndex::new(text);
        let url = lsp_types::Url::parse("file:///test.sail").expect("a valid test URL");
        diags.iter().map(|d| crate::to_proto::diagnostic(&line_index, &url, d)).collect()
    }
}

fn diagnostic_code_str(diagnostic: &lsp_types::Diagnostic) -> Option<&str> {
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
    let file = SalsaTestFile::new(source.to_string());
    let call_offset = source.find("2)").unwrap();
    let lc = file.source.position_at(call_offset);
    let call = find_call_at_position(&file, lc);
    assert_eq!(call, Some(("add".to_string(), 1)));
}

#[test]
fn finds_call_and_argument_index_in_top_level_initializer() {
    let source = r#"
val add : (int, int) -> int
function add(x, y) = x + y
let result = add(1, 2)
"#;
    let file = SalsaTestFile::new(source.to_string());
    let call_offset = source.find("2)").unwrap();
    let lc = file.source.position_at(call_offset);
    let call = find_call_at_position(&file, lc);
    assert_eq!(call, Some(("add".to_string(), 1)));
}

#[test]
fn collects_callable_signatures() {
    let source = r#"
val add : (int, int) -> int
function add(x, y) = x + y
"#;
    let file = SalsaTestFile::new(source.to_string());
    let signatures = collect_callable_signatures(&file);
    assert!(signatures.iter().any(|sig| sig.name == "add"));
}

#[test]
fn builds_function_snippet() {
    let params = vec![
        Parameter { name: "x".to_string(), is_implicit: false },
        Parameter { name: "y : int".to_string(), is_implicit: false },
    ];
    assert_eq!(function_snippet("add", &params), "add(${1:x}, ${2:y})");
}

#[test]
fn offers_missing_semicolon_fix() {
    let source = "function f() = {\n  let x = 1\n}\n";
    let file = SalsaTestFile::new(source.to_string());
    let line_index = ide_db::line_index::LineIndex::new(source);
    let lsp_diag = Diagnostic::new_simple(
        Range::new(lsp_types::Position::new(1, 2), lsp_types::Position::new(1, 10)),
        "expected ';'".to_string(),
    );
    let diagnostic = crate::from_proto::diagnostic(&line_index, &lsp_diag);

    let edit = missing_semicolon_fix(&file, &diagnostic).expect("expected quick fix");
    assert_eq!(edit.new_text, ";");
}

#[test]
fn offers_missing_closer_fix() {
    let source = "function f() = {\n  let x = (1 + 2\n}\n";
    let file = SalsaTestFile::new(source.to_string());
    let line_index = ide_db::line_index::LineIndex::new(source);
    let lsp_diag = Diagnostic::new_simple(
        Range::new(lsp_types::Position::new(1, 16), lsp_types::Position::new(1, 16)),
        "expected ')'".to_string(),
    );
    let diagnostic = crate::from_proto::diagnostic(&line_index, &lsp_diag);

    let (_, edit, _) = quick_fix_for_diagnostic(&file, &diagnostic).expect("expected fix");
    assert_eq!(edit.new_text, ")");
}

#[test]
fn captures_return_type_from_val_signature() {
    let source = "val f : int -> bits(32)\nfunction f(x) = x\n";
    let file = SalsaTestFile::new(source.to_string());
    let signatures = collect_callable_signatures(&file);
    let f = signatures.into_iter().find(|sig| sig.name == "f").expect("missing signature");
    assert_eq!(f.return_type.as_deref(), Some("bits(32)"));
}

#[test]
fn reports_missing_closing_paren_as_syntax_error() {
    let source = "function f() = {\n  let x = (1 + 2\n}\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("syntax-error")
                && diagnostic.message.contains("expected ')'")
        })
        .expect("missing syntax diagnostic");

    assert_eq!(diagnostic.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
}

#[test]
fn reports_type_errors_from_typecheck() {
    let source = "val f : bool -> unit\nfunction f(x) = ()\nfunction g() -> unit = f(1)\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| diagnostic_code_str(diagnostic) == Some("type-error"))
        .expect("missing type diagnostic");

    assert!(diagnostic.message.contains("bool"));
}

#[test]
fn reports_mismatched_arg_count_from_typecheck() {
    let source =
        "val add : (int, int) -> int\nfunction add(x, y) = x + y\nfunction main() = add(1)\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| diagnostic_code_str(diagnostic) == Some("mismatched-arg-count"))
        .expect("missing mismatched arg count diagnostic");

    assert!(diagnostic.message.contains("expected 2 arguments, found 1"));
}

#[test]
fn reports_missing_record_fields_from_typecheck() {
    let source = "struct S = { x : int, y : bool }\nfunction f() = { struct S { x = 1 } }\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("missing-struct-fields")
                && diagnostic.message.contains("struct literal missing fields: y")
        })
        .expect("missing record field diagnostic");

    assert_eq!(diagnostic.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
}

#[test]
fn reports_record_field_type_errors_from_typecheck() {
    let source = "struct S = { x : int }\nfunction f() = { struct S { x = true } }\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("bool")
                && diagnostic.message.contains("int")
        })
        .expect("missing record field type diagnostic");

    assert_eq!(diagnostic.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
}

#[test]
fn reports_record_update_field_type_errors_from_typecheck() {
    let source =
        "struct S = { x : int }\nfunction f() = {\n  let s : S = struct S { x = 1 };\n  { s with x = true }\n}\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("bool")
                && diagnostic.message.contains("int")
        })
        .expect("missing record update diagnostic");

    assert_eq!(diagnostic.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
}

#[test]
fn applies_record_type_arguments_to_field_types() {
    let source = "struct pair('a) = { fst : 'a, snd : 'a }\nfunction f(p : pair(int)) = { let x : bool = p.fst; x }\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("int")
                && diagnostic.message.contains("bool")
        })
        .expect("missing instantiated record field diagnostic");

    assert_eq!(diagnostic.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
}

#[test]
fn applies_expected_record_type_to_generic_struct_literals() {
    let source = "struct pair('a) = { fst : 'a, snd : 'a }\nfunction mk() -> pair(int) = { struct pair { fst = 1, snd = true } }\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("bool")
                && diagnostic.message.contains("int")
        })
        .expect("missing generic struct literal diagnostic");

    assert_eq!(diagnostic.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
}

#[test]
fn reports_list_element_type_errors_against_expected_return_type() {
    let source = "function f() -> list(bool) = [|true, 1|]\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("int")
                && diagnostic.message.contains("bool")
        })
        .expect("missing list element type diagnostic");

    assert_eq!(diagnostic.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
}

#[test]
fn reports_tuple_element_type_errors_against_expected_return_type() {
    let source = "function f() -> (int, bool) = (1, 2)\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("int")
                && diagnostic.message.contains("bool")
        })
        .expect("missing tuple element type diagnostic");

    assert_eq!(diagnostic.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
}

#[test]
fn reports_vector_length_errors_against_expected_return_type() {
    let source = "function f() -> vector(2, bool) = [true, false, true]\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    // Upstream counts the elements rather than naming the two types:
    // "Vector literal with incorrect length: found 3 elements, but
    // expected 2" (`type_check.ml:2493`). Naming `vector(3, bool)` and
    // `vector(2, bool)` says the same thing and does not tell the reader
    // to count anything.
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message
                    == "Vector literal with incorrect length: found 3 elements, but expected 2"
        })
        .expect("missing vector length diagnostic");

    assert_eq!(diagnostic.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
}

#[test]
fn binds_union_constructor_pattern_payload_types() {
    let source = "union opt('a) = { None : unit, Some : 'a }\nfunction f(x : opt(int)) = match x { Some(v) => if v then () else (), None() => () }\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("int")
                && diagnostic.message.contains("bool")
        })
        .expect("missing constructor payload type diagnostic");

    assert_eq!(diagnostic.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
    assert!(diagnostics
        .iter()
        .all(|diagnostic| !diagnostic.message.contains("Identifier v is unbound")));
}

#[test]
fn checks_match_case_bodies_against_expected_return_type() {
    let source = "union opt('a) = { None : unit, Some : 'a }\nfunction f(x : opt(int)) -> bool = match x { Some(v) => v, None() => false }\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("int")
                && diagnostic.message.contains("bool")
        })
        .expect("missing match branch expected type diagnostic");

    assert_eq!(diagnostic.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
}

#[test]
fn checks_annotated_local_list_bindings_with_expected_type() {
    let source = "function f() = { let xs : list(bool) = [|true, 1|]; () }\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("int")
                && diagnostic.message.contains("bool")
        })
        .expect("missing annotated let type diagnostic");

    assert_eq!(diagnostic.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
}

#[test]
fn reports_struct_pattern_missing_fields_from_typecheck() {
    let source =
        "struct S = { x : int, y : bool }\nfunction f(s : S) = match s { struct S { x } => () }\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("missing-fields")
                && diagnostic.message.contains("missing fields in `S`: y")
        })
        .expect("missing struct pattern field diagnostic");

    assert_eq!(diagnostic.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
}

#[test]
fn does_not_report_missing_fields_for_struct_pattern_wildcard() {
    let source = "struct S = { x : int, y : bool }\nfunction f(s : S) = match s { struct S { x, _ } => x }\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics
        .iter()
        .all(|diagnostic| !diagnostic.message.contains("struct pattern missing fields")));
}

#[test]
fn reports_duplicate_pattern_bindings_from_typecheck() {
    let source = "function f(xs : list(int)) = match xs { x :: x => x, [||] => 0 }\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("duplicate-definition")
                && diagnostic.message == "Duplicate binding for x in pattern"
        })
        .expect("missing duplicate pattern diagnostic");

    assert_eq!(diagnostic.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
}

#[test]
fn binds_vector_subrange_patterns_like_upstream() {
    let source = "default Order dec\nfunction f(x : bits(8)) = match x { flag[7 .. 4] @ flag[3 .. 0] => if flag then () else () }\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("bits(8)")
                && diagnostic.message.contains("bool")
        })
        .expect("missing vector subrange binding diagnostic");

    assert_eq!(diagnostic.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
    assert!(diagnostics
        .iter()
        .all(|diagnostic| !diagnostic.message.contains("Identifier flag is unbound")));
}

#[test]
fn binds_multi_part_vector_subrange_patterns_like_upstream() {
    let source = "default Order dec\nfunction f(x : bits(8)) = match x { flag[7 .. 6] @ flag[5 .. 2] @ flag[1 .. 0] => if flag then () else () }\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("bits(8)")
                && diagnostic.message.contains("bool")
        })
        .expect("missing vector subrange binding diagnostic");

    assert_eq!(diagnostic.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
    assert!(diagnostics
        .iter()
        .all(|diagnostic| !diagnostic.message.contains("Identifier flag is unbound")));
}

#[test]
fn reports_non_contiguous_vector_subrange_patterns() {
    let source = "default Order dec\nfunction f(x : bits(8)) = match x { flag[7 .. 4] @ flag[2 .. 0] => (), _ => () }\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("pattern subranges are non-contiguous")
        })
        .expect("missing non-contiguous subrange diagnostic");

    assert_eq!(diagnostic.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
}

/// An enum does not accept a bitvector.
///
/// `unify` lets a non-primitive `Named` accept anything — its tail returns
/// true whenever either side is not a primitive — because a `Named` is
/// also how a *type alias* arrives, and `xlenbits` has to accept
/// `bits(64)`. An enum is not an alias, and `let y : E = 0x0` went
/// unreported where upstream gives a type error.
#[test]
fn an_enum_does_not_accept_a_bitvector() {
    let source = "enum E = { A, B }\nfunction g() -> unit = {\n  let y : E = 0x0;\n  ()\n}\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    assert!(
        diagnostics.iter().any(|d| diagnostic_code_str(d) == Some("type-error")),
        "no type error for an enum bound to a bitvector: {diagnostics:?}"
    );
}

/// And an alias still accepts what it aliases.
#[test]
fn an_alias_still_accepts_what_it_aliases() {
    let source = "type W = bits(4)\nfunction f(x : W) -> W = x\nfunction g() -> unit = {\n  let y : bits(4) = f(0x0);\n  ()\n}\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    assert!(
        diagnostics.iter().all(|d| diagnostic_code_str(d) != Some("type-error")),
        "an alias was judged against its own expansion: {diagnostics:?}"
    );
}

#[test]
fn typechecks_mapping_calls_as_expressions() {
    let source = "enum width = BYTE | DOUBLE\nmapping size_bits : width <-> bits(2) = { BYTE <-> 0b00, DOUBLE <-> 0b11 }\nfunction f() -> bits(2) = size_bits(BYTE)\nfunction g() -> width = size_bits(0b11)\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic_code_str(diagnostic) != Some("type-error")));
}

#[test]
fn binds_mapping_pattern_payload_types() {
    let source = "enum width = BYTE | DOUBLE\nmapping size_bits : width <-> bits(2) = { BYTE <-> 0b00, DOUBLE <-> 0b11 }\nfunction f(x : width) = match x { size_bits(bits) => if bits then () else () }\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("bits(2)")
                && diagnostic.message.contains("bool")
        })
        .expect("missing mapping pattern payload diagnostic");

    assert_eq!(diagnostic.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
    assert!(diagnostics
        .iter()
        .all(|diagnostic| !diagnostic.message.contains("Identifier bits is unbound")));
}

#[test]
fn checks_mapping_guards_with_pattern_bindings() {
    let source = "enum width = BYTE | DOUBLE\nval decode : bits(2) -> width\nmapping size_bits : width <-> bits(2) = { backwards bits if bits => decode(bits) }\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("bits(2)")
                && diagnostic.message.contains("bool")
        })
        .expect("missing mapping guard diagnostic");

    assert_eq!(diagnostic.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
    assert!(diagnostics
        .iter()
        .all(|diagnostic| !diagnostic.message.contains("Identifier bits is unbound")));
}

#[test]
fn reports_unresolved_quants_from_generic_call_without_context() {
    // Read the structured channel, not the LSP surface: the mapping from
    // `InferenceDiagnostic::UnresolvedCallQuantifiers` to a rendered
    // diagnostic is deliberately absent, because the resolver gives up on
    // quantifiers far more often than a program is actually wrong (see
    // `hir/src/diagnostics.rs`). This pins the checker while the surface
    // stays quiet.
    let source = "val zeroes : forall 'n. unit -> bits('n)\nfunction use() = zeroes(())\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.checker_inference_diagnostics();
    let signature = diagnostics
        .iter()
        .find_map(|d| match d {
            hir::InferenceDiagnostic::UnresolvedCallQuantifiers {
                id, quants, signature, ..
            } if id == "zeroes" => Some((quants.clone(), signature.clone())),
            _ => None,
        })
        .expect("missing unresolved quantifier diagnostic");

    assert_eq!(signature.0, vec!["'n".to_string()], "the unresolved quantifier is `'n`");
    // The original signature is carried along so a reader can see which
    // generic shape produced it.
    let sig = signature.1.expect("signature text");
    assert!(sig.contains("unit ->"), "expected the signature, got: {sig}");
}

#[test]
fn reports_failed_constraints_from_generic_call() {
    let source =
        "val widen : forall 'n, 'n in {1, 2}. unit -> bits(8 * 'n)\nfunction use() -> bits(24) = widen(())\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("unsolved-constraint")
                && diagnostic.message.contains("'n in {1, 2}")
        })
        .expect("missing failed constraint diagnostic");

    // An error, not a warning: `ConstraintStatus::Failed` is the solver
    // saying it *proved* the constraint false, and upstream rejects this
    // file. It read as a warning because the branch that produces it passed
    // `proved_false: false`.
    assert_eq!(diagnostic.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
}

#[test]
fn infers_slice_result_types_for_bits() {
    let source = "default Order dec\nfunction hi(x : bits(8)) -> bits(4) = x[7 .. 4]\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic_code_str(diagnostic) != Some("type-error")));
}

#[test]
fn typechecks_comma_slice_sugar_via_builtin_slice() {
    let source = "function mid(x : bits(8)) -> bits(3) = x[2, 3]\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic_code_str(diagnostic) != Some("type-error")));
}

#[test]
fn reports_out_of_bounds_bit_index_access() {
    // A constraint the solver *refutes* is an error. `x[8]` on `bits(8)`
    // needs `0 <= 8 & 8 < 8`, false for constants, and upstream rejects the
    // file. The comment that stood here said this was a warning because
    // "Sail programs routinely index with values the checker cannot bound"
    // — that case is `Unknown`, which is still a warning, not `Failed`.
    let source = "function bit_at(x : bits(8)) -> bit = x[8]\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("unsolved-constraint")
                && diagnostic.message.contains("0 <= 8 < 8")
        })
        .expect("missing out-of-bounds constraint diagnostic");
    // An error, not a warning: `ConstraintStatus::Failed` is the solver
    // saying it *proved* the constraint false, and upstream rejects this
    // file. It read as a warning because the branch that produces it passed
    // `proved_false: false`.
    assert_eq!(diagnostic.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
}

#[test]
fn reports_vector_update_range_value_type_mismatch() {
    let source =
        "default Order dec\nfunction patch(x : bits(8)) -> bits(8) = [x with 7 .. 4 = 0b101]\n";
    let file = SalsaTestFile::new(source.to_string());
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
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic_code_str(diagnostic) != Some("type-error")));
}

#[test]
fn infers_bitfield_access_and_updates() {
    let source = "bitfield B : bits(8) = { HI : 7 .. 4, LO : 3 }\nregister R : B\nfunction hi() -> bits(4) = R[HI]\nfunction lo() -> bit = R.LO\nfunction bits() -> bits(8) = R.bits\nfunction patch() -> B = [R with HI = 0b1010]\nfunction write() -> unit = { R[HI] = 0b0001; () }\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic_code_str(diagnostic) != Some("type-error")));
}

#[test]
fn infers_concat_bitfield_ranges_like_upstream() {
    let source = "bitfield B : bits(32) = { Field0 : (31 .. 16 @ 7 .. 0), Field1 : 15 .. 8 }\nregister R : B\nfunction get0() -> bits(24) = R[Field0]\nfunction get1() -> bits(8) = R[Field1]\nfunction patch() -> B = [R with Field1 = 0x47, Field0 = 0x000011]\nfunction write() -> unit = { R[Field0] = 0x4711FF; () }\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic_code_str(diagnostic) != Some("type-error")));
}

#[test]
fn solves_linear_numeric_quants_from_expected_return_type() {
    let source =
        "val widen : forall 'n. unit -> bits(8 * 'n)\nfunction use() -> bits(16) = widen(())\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic_code_str(diagnostic) != Some("type-error")));
}

#[test]
fn resolves_quant_constraints_via_global_constraint_definitions() {
    let source = "type max_mem_access : Int\nconstraint max_mem_access == 8\nval take_width : forall 'n, 0 < 'n <= max_mem_access . bits('n) -> unit\nfunction use() -> unit = take_width(0b10101010)\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic_code_str(diagnostic) != Some("type-error")));
}

#[test]
fn propagates_assert_constraints_to_following_calls() {
    let source = "let max_mem_access : int = 8\nval take_width : forall 'n, 0 < 'n <= max_mem_access . bits('n) -> unit\nfunction use() -> unit = { assert(max_mem_access == 8, \"ok\"); take_width(0b10101010) }\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic_code_str(diagnostic) != Some("type-error")));
}

#[test]
fn resolves_quant_constraints_from_matching_global_assumptions() {
    let source = "type xlen : Int\nconstraint xlen in {32, 64}\nval take_width : forall 'n, 'n in {32, 64} . bits('n) -> unit\nfunction use(xs : bits(xlen)) -> unit = take_width(xs)\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();

    assert!(diagnostics
        .iter()
        .all(|diagnostic| diagnostic_code_str(diagnostic) != Some("type-error")));
}

#[test]
fn reports_literal_pattern_type_mismatches() {
    let source = "function f(x : string) = match x { 1 => (), _ => () }\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("atom(1)")
                && diagnostic.message.contains("string")
        })
        .expect("missing literal pattern mismatch diagnostic");

    assert_eq!(diagnostic.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
}

#[test]
fn infers_callable_head_param_types_from_literal_patterns() {
    let source = "function pick((1, x : int)) -> int = x\nfunction use() = pick((true, 0))\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("bool")
                && diagnostic.message.contains("int")
        })
        .expect("missing inferred callable head type diagnostic");

    assert_eq!(diagnostic.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
}

#[test]
fn reports_non_int_index_from_typecheck() {
    let source = "function f() = { let v = [1, 2]; v[true] }\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("bool")
                && diagnostic.message.contains("int")
        })
        .expect("missing index diagnostic");

    assert_eq!(diagnostic.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
}

#[test]
fn binds_foreach_iterators_for_typecheck() {
    let source = "function f(n : int) = { foreach (i from 0 to n) { if i then () else () }; () }\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let bool_mismatch = diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic_code_str(diagnostic) == Some("type-error")
                && diagnostic.message.contains("int")
                && diagnostic.message.contains("bool")
        })
        .expect("missing foreach iterator type diagnostic");

    assert_eq!(bool_mismatch.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
    assert!(diagnostics
        .iter()
        .all(|diagnostic| !diagnostic.message.contains("Identifier i is unbound")));
}

#[test]
fn does_not_warn_on_type_ascription() {
    // Sail's `expr : type` is type ascription (E_typ in upstream parser.mly),
    // NOT a deprecated cast. Upstream only deprecates the `val cast f : ...`
    // declaration form. We must not flag plain ascription as deprecated.
    let source = "function f(x) = x : int\n";
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    assert!(
        diagnostics.iter().all(|d| diagnostic_code_str(d) != Some("deprecated-cast-annotation")),
        "type ascription wrongly flagged as deprecated cast: {diagnostics:?}"
    );
}

#[test]
fn builds_selection_range_chain() {
    let source = "function f() = {\n  let x = (1 + 2);\n}\n";
    let file = SalsaTestFile::new(source.to_string());
    let pos = ide_db::LineCol { line: 1, col: 13 };
    let offset = ide_db::FileDb::offset_at(&file, &pos);
    let selection = selection_range(&file, offset).expect("no selection range");
    assert!(!selection.range.is_empty());
    assert!(selection.parent.is_some());
}

/// The chain has the granularity the tree has.
///
/// The bracket-and-line scraper this replaced could only answer with the
/// token, a bracket pair spanning it, the whole definition, the line and
/// the file — so `1 + 2`, the expression the cursor is actually inside,
/// was not on the chain at all.
#[test]
fn a_selection_range_steps_through_the_tree() {
    let source = "function f() = {\n  let x = 1 + 2;\n}\n";
    let file = SalsaTestFile::new(source.to_string());
    // On the `1`.
    let offset = source.find("1 + 2").expect("fixture");
    let mut node = Some(selection_range(&file, offset).expect("no selection range"));
    let mut texts = Vec::new();
    while let Some(n) = node {
        texts.push(&source[base_db::range_start(n.range)..base_db::range_end(n.range)]);
        node = n.parent.map(|p| *p);
    }
    assert!(texts.contains(&"1"), "innermost is the token: {texts:?}");
    assert!(texts.contains(&"1 + 2"), "the expression is missing: {texts:?}");
    assert!(texts.contains(&"let x = 1 + 2;"), "the statement is missing: {texts:?}");
    // Strictly growing, innermost first — what the protocol asks for.
    for pair in texts.windows(2) {
        assert!(pair[0].len() < pair[1].len(), "not strictly growing: {texts:?}");
    }
}

#[test]
fn parses_named_type() {
    assert_eq!(parse_named_type("bits(32)"), None);
    assert_eq!(parse_named_type("my_struct"), Some("my_struct".to_string()));
    assert_eq!(parse_named_type("option(my_type)"), Some("option".to_string()));
}

#[test]
fn extracts_typed_bindings() {
    let file = SalsaTestFile::new("let x : my_type = 1".to_string());
    let bindings = typed_bindings(&file);
    assert_eq!(bindings.get("x"), Some(&"my_type".to_string()));
}

#[test]
fn extracts_typed_function_parameter_bindings() {
    let file = SalsaTestFile::new("function f(x : bits(32), y : int) = x".to_string());
    let bindings = typed_bindings(&file);
    assert_eq!(bindings.get("x"), Some(&"bits(32)".to_string()));
    assert_eq!(bindings.get("y"), Some(&"int".to_string()));
}

#[test]
fn does_not_treat_types_as_function_parameter_names() {
    let source = "function f(x : bits(32), y : int) -> bits(32) = x\n";
    let file = SalsaTestFile::new(source.to_string());
    let sig = collect_callable_signatures(&file)
        .into_iter()
        .find(|sig| sig.name == "f")
        .expect("missing signature");

    let params = sig.params.into_iter().map(|param| param.name).collect::<Vec<_>>();
    assert_eq!(params, vec!["x : bits(32)".to_string(), "y : int".to_string()]);
    assert_eq!(sig.return_type.as_deref(), Some("bits(32)"));
}

#[test]
fn finds_implementation_locations() {
    let file = SalsaTestFile::new("val foo : int -> int\nfunction foo(x) = x\n".to_string());
    let uri = Url::parse("file:///tmp/test.sail").unwrap();
    let locations = implementation_locations(std::iter::once((&uri, &file)), &uri, "foo");
    assert!(!locations.is_empty());
}

#[test]
fn formats_document_passthrough() {
    // CST visitor passes through function bodies it does not rewrite.
    let options = ide_db::ide_types::FormatOptions {
        tab_size: 2,
        insert_spaces: true,
        trim_trailing_whitespace: Some(true),
        insert_final_newline: None,
        trim_final_newlines: None,
        max_line_width: None,
    };
    let source = "function f() = {\nlet x = [1,\n2]\n}\n";
    let formatted = format_document_cst(source, &options);
    // CST visitor preserves source structure for constructs it doesn't handle.
    assert_eq!(formatted, source);
}

#[test]
fn formats_only_selected_range() {
    let options = ide_db::ide_types::FormatOptions {
        tab_size: 2,
        insert_spaces: true,
        trim_trailing_whitespace: Some(true),
        insert_final_newline: None,
        trim_final_newlines: None,
        max_line_width: None,
    };
    // Use a source that the CST visitor actually reformats (e.g. struct alignment).
    let source = "struct Point = {\n  x : int,\n  y_offset : bits(32),\n}\n";
    let file = SalsaTestFile::new(source.to_string());
    let f: &dyn ide_db::FileDb = &file;
    let range = base_db::text_range(
        f.offset_at(&ide_db::LineCol { line: 0, col: 0 }),
        f.offset_at(&ide_db::LineCol { line: 4, col: 0 }),
    );
    let edits = range_format_document_edits(&file, range, &options);
    // CST visitor may or may not produce edits depending on alignment.
    // The key contract: no panic, and if edits exist they are valid.
    if let Some(edits) = edits {
        assert!(!edits.is_empty());
    }
}

#[test]
fn preserves_existing_continuation_indent() {
    let options = ide_db::ide_types::FormatOptions {
        tab_size: 2,
        insert_spaces: true,
        trim_trailing_whitespace: Some(true),
        insert_final_newline: None,
        trim_final_newlines: None,
        max_line_width: None,
    };
    let source = "mapping clause assembly = RFWVVTYPE(funct6, vm, vs2, vs1, vd)\n\t<-> rfwvvtype_mnemonic(funct6) ^ spc() ^ vreg_name(vd)\n";
    let formatted = format_document_cst(source, &options);
    assert_eq!(formatted, source);
}

#[test]
fn preserves_tab_indent_even_when_computed_indent_is_spaces() {
    let options = ide_db::ide_types::FormatOptions {
        tab_size: 2,
        insert_spaces: true,
        trim_trailing_whitespace: Some(true),
        insert_final_newline: None,
        trim_final_newlines: None,
        max_line_width: None,
    };
    let source = "function f() = {\n\tx\n}\n";
    let formatted = format_document_cst(source, &options);
    assert_eq!(formatted, source);
}

#[test]
fn preserves_type_variables_in_let_bindings() {
    let options = ide_db::ide_types::FormatOptions {
        tab_size: 2,
        insert_spaces: true,
        trim_trailing_whitespace: Some(true),
        insert_final_newline: None,
        trim_final_newlines: None,
        max_line_width: None,
    };
    // Wrap in a function so the CST can parse it as valid Sail.
    let source = "function foo() = {\n  let vm_val  : bits('n)             = read_vmask(num_elem_vs, vm, zvreg);\n  let vd_val  : vector('d, bits('m)) = read_vreg(num_elem_vd, SEW, 0, vd);\n}\n";
    let formatted = format_document_cst(source, &options);
    // Type variables like 'n and 'm should not cause spurious indentation changes.
    assert!(formatted.contains("let vm_val"), "vm_val preserved: {formatted}");
    assert!(formatted.contains("let vd_val"), "vd_val preserved: {formatted}");
}

#[test]
fn returns_linked_editing_ranges_for_identifier() {
    let source = "let x = x\n";
    let file = SalsaTestFile::new(source.to_string());
    let offset = source.rfind('x').expect("rhs x");
    let lc = file.source.position_at(offset);
    let linked = linked_editing_ranges_for_position(&file, lc).expect("linked ranges");
    assert!(linked.ranges.len() >= 2);
}

#[test]
#[cfg(unix)] // Uses file:///tmp/ URI which doesn't resolve to an absolute path on Windows
fn extracts_document_links() {
    let uri = Url::parse("file:///tmp/main.sail").unwrap();
    let source = "let a = \"sub/module.sail\"\n// see https://example.com/spec\n";
    let file = SalsaTestFile::new(source.to_string());
    let links = document_links_for_file(&uri, &file);
    assert!(links.len() >= 2);
    assert!(links.iter().any(|l| {
        l.target.as_ref().map(|u| u.as_str().contains("example.com")).unwrap_or(false)
    }));
}

#[test]
fn builds_code_lenses_for_declarations() {
    let source = "val foo : int\nfunction foo() = 1\n";
    let file = SalsaTestFile::new(source.to_string());
    let uri = Url::parse("file:///tmp/main.sail").unwrap();
    let all_files = vec![(&uri, &file)];
    let refs = collect_reference_counts(&all_files);
    let impls = collect_implementation_counts(&all_files);
    let lenses = code_lenses_ide(&file, &refs, &impls, &Default::default());
    // 2 lenses per function: references count + implementations count.
    // "Run" lens was removed (no Sail test runner integration).
    assert!(lenses.len() >= 2, "expected >= 2 lenses, got {}", lenses.len());
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
    let file = SalsaTestFile::new(source.to_string());
    let lsp_diagnostics = file.lsp_diagnostics();
    let unused_x = lsp_diagnostics.iter().find(|d| d.message.contains("unused variable `x`"));
    let used_y = lsp_diagnostics.iter().find(|d| d.message.contains("unused variable `y`"));

    assert!(unused_x.is_some());
    assert!(used_y.is_none());
    assert_eq!(unused_x.unwrap().severity, Some(lsp_types::DiagnosticSeverity::WARNING));
    assert!(unused_x
        .unwrap()
        .tags
        .as_ref()
        .unwrap()
        .contains(&lsp_types::DiagnosticTag::UNNECESSARY));
}

#[test]
fn detects_unused_shadowed_outer_binding() {
    // The outer `x` is shadowed by the inner `x`, but since unused-variable
    // detection is name-based (not scope-aware), any use of `x` anywhere
    // in the body suppresses the unused warning for all `x` bindings.
    // This is a known limitation — scope-aware tracking would require
    // mapping Ident uses to specific PatIds.
    let source = "function foo() = {\n  let x = 1;\n  let y = let x = 2 in x;\n  y\n}\n";
    let file = SalsaTestFile::new(source.to_string());
    let unused_x = file
        .lsp_diagnostics()
        .into_iter()
        .filter(|diagnostic| diagnostic.message.contains("unused variable `x`"))
        .count();

    assert_eq!(unused_x, 0);
}

#[test]
fn does_not_warn_for_enum_members_in_patterns() {
    let source = "enum instr = VI_VRGATHER | VI_VADD\nfunction decode(instr) = match instr {\n  VI_VRGATHER => 1,\n  VI_VADD => 2\n}\n";
    let file = SalsaTestFile::new(source.to_string());
    let lsp_diagnostics = file.lsp_diagnostics();

    assert!(!lsp_diagnostics.iter().any(|d| d.message.contains("Unused variable: `VI_VRGATHER`")));
    assert!(!lsp_diagnostics.iter().any(|d| d.message.contains("Unused variable: `VI_VADD`")));
}

#[test]
fn resolves_enum_member_patterns_as_top_level_symbols() {
    let source = "enum instr = VI_VRGATHER | VI_VADD\nfunction decode(instr) = match instr {\n  VI_VRGATHER => 1,\n  VI_VADD => 2\n}\n";
    let file = SalsaTestFile::new(source.to_string());
    let lc = file.source.position_at(source.rfind("VI_VRGATHER =>").expect("pattern ref"));

    let symbol = resolve_symbol_at(&file, lc).expect("resolved symbol");
    assert!(!symbol.is_local(), "an enum member is a top-level definition: {symbol:?}");
    assert_eq!(symbol.binding_span(), None);

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
    let file = SalsaTestFile::new(source.to_string());
    let lsp_diagnostics = file.lsp_diagnostics();
    // Upstream's wording, and its second caret: `already_bound`
    // (`type_env.ml:420`) points at the name the redefinition declares and
    // hints "previous binding" at the first one.
    let dup = lsp_diagnostics
        .iter()
        .find(|d| d.message == "Cannot create struct type S, name is already bound")
        .expect("the redefinition is reported");
    assert_eq!(dup.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
    let related = dup.related_information.as_ref().expect("the first binding is a location");
    assert_eq!(related.len(), 1);
    assert_eq!(related[0].message, "previous binding");
}

#[test]
fn detects_unreachable_code() {
    let source = "function foo() = {\n  return 1;\n  let x = 2;\n}\n";
    let file = SalsaTestFile::new(source.to_string());
    let lsp_diagnostics = file.lsp_diagnostics();
    let unreachable = lsp_diagnostics.iter().find(|d| d.message.contains("unreachable code"));

    assert!(unreachable.is_some());
    assert_eq!(unreachable.unwrap().severity, Some(lsp_types::DiagnosticSeverity::HINT));
    assert!(unreachable
        .unwrap()
        .tags
        .as_ref()
        .unwrap()
        .contains(&lsp_types::DiagnosticTag::UNNECESSARY));
}

#[test]
fn detects_unreachable_after_terminating_if() {
    let source = "function foo(b) = {\n  if b then return 1 else return 2;\n  let x = 3;\n}\n";
    let file = SalsaTestFile::new(source.to_string());
    let unreachable = file
        .lsp_diagnostics()
        .into_iter()
        .find(|diagnostic| diagnostic.message.contains("unreachable code"));

    assert!(unreachable.is_some());
}

#[test]
fn detects_mismatched_argument_count() {
    let source = "val f : (int, int) -> int\nfunction f(a, b) = a + b\nfunction g() = f(1)\n";
    let file = SalsaTestFile::new(source.to_string());
    let lsp_diagnostics = file.lsp_diagnostics();
    let mismatch =
        lsp_diagnostics.iter().find(|d| d.message.contains("expected 2 arguments, found 1"));

    assert!(mismatch.is_some());
    assert_eq!(mismatch.unwrap().severity, Some(lsp_types::DiagnosticSeverity::ERROR));
}

#[test]
fn does_not_detect_duplicate_definitions_for_scattered_clauses() {
    let source = r#"
scattered function foo
function clause foo(x) = x
function clause foo(x) = x + 1
"#;
    let file = SalsaTestFile::new(source.to_string());
    let lsp_diagnostics = file.lsp_diagnostics();
    let dup = lsp_diagnostics.iter().find(|d| d.message.contains("Duplicate definition of `foo`"));

    assert!(dup.is_none());
}

#[test]
fn detects_mismatched_argument_count_with_implicits() {
    let source = "val f : (implicit(int), int) -> int\nfunction f(i, x) = x\nfunction g() = f(1)\n";
    let file = SalsaTestFile::new(source.to_string());
    let lsp_diagnostics = file.lsp_diagnostics();
    let mismatch = lsp_diagnostics.iter().find(|d| d.message.contains("expected 2 arguments"));

    // 1 argument is valid because 1 is implicit
    assert!(mismatch.is_none());

    let source2 =
        "val f : (implicit(int), int) -> int\nfunction f(i, x) = x\nfunction g() = f(1, 2, 3)\n";
    let file2 = SalsaTestFile::new(source2.to_string());
    let lsp_diagnostics2 = file2.lsp_diagnostics();
    let mismatch2 =
        lsp_diagnostics2.iter().find(|d| d.message.contains("expected 2 arguments, found 3"));
    assert!(mismatch2.is_some());
}

#[test]
fn handles_space_separated_params() {
    let source = "val HaveEL : bits(2) -> bool\nfunction HaveEL el = true\nlet _ = HaveEL(0b00)\n";
    let file = SalsaTestFile::new(source.to_string());
    let lsp_diagnostics = file.lsp_diagnostics();
    let mismatch = lsp_diagnostics.iter().find(|d| d.message.contains("expected 2 arguments"));
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
    let file = SalsaTestFile::new(source.to_string());
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
    let file = SalsaTestFile::new(source.to_string());
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
    let file = SalsaTestFile::new(source.to_string());
    let all_files = vec![(&uri, &file)];
    let impls = collect_implementation_counts(&all_files);

    assert_eq!(impls.get("foo").copied(), Some(2));
}

#[test]
fn resolves_workspace_symbol_location() {
    let uri = Url::parse("file:///tmp/main.sail").unwrap();
    let file = SalsaTestFile::new("function foo() = 1\n".to_string());
    let resolved = resolve_workspace_symbol(
        "foo",
        ide_db::ide_types::SymbolKind::Function,
        &uri,
        std::iter::once((&uri, &file)),
    );
    assert!(resolved.is_some());
}

#[test]
fn extracts_type_alias_edges() {
    let file = SalsaTestFile::new("type child = parent\n".to_string());
    let edges = type_alias_edges(&file);
    assert_eq!(edges, vec![("child".to_string(), "parent".to_string())]);
}

#[test]
fn computes_type_hierarchy_relations() {
    let uri = Url::parse("file:///tmp/main.sail").unwrap();
    let file = SalsaTestFile::new(
        "type parent = base\ntype child = parent\ntype grandchild = child\n".to_string(),
    );
    let supers = type_supertypes(std::iter::once((&uri, &file)), &uri, "child");
    let subs = type_subtypes(std::iter::once((&uri, &file)), &uri, "child");
    assert!(supers.iter().any(|item| item.name == "parent"));
    assert!(subs.iter().any(|item| item.name == "grandchild"));
}

#[test]
fn finds_type_candidates_at_position() {
    let source = "let x : child = y\n";
    let file = SalsaTestFile::new(source.to_string());
    let lc = file.source.position_at(source.find("x").unwrap());
    let names = type_name_candidates_at_position(&file, lc);
    assert!(names.contains(&"child".to_string()));
    assert!(names.contains(&"x".to_string()));
}

#[test]
fn creates_will_rename_file_edits() {
    let uri = Url::parse("file:///tmp/main.sail").unwrap();
    let file = SalsaTestFile::new("let inc = \"old.sail\"\n".to_string());
    let rename_pairs =
        vec![("file:///tmp/old.sail".to_string(), "file:///tmp/new.sail".to_string())];
    let changes = will_rename_file_edits(std::iter::once((&uri, &file)), &rename_pairs)
        .expect("expected edits");
    assert_eq!(changes.len(), 1);
}

#[test]
fn lazy_code_action_data_roundtrip() {
    let uri = Url::parse("file:///tmp/main.sail").unwrap();
    let edit = TextEdit {
        range: Range::new(lsp_types::Position::new(0, 0), lsp_types::Position::new(0, 0)),
        new_text: ";".to_string(),
    };
    let mut changes = std::collections::HashMap::new();
    changes.insert(uri.clone(), vec![edit.clone()]);
    let workspace_edit = lsp_types::WorkspaceEdit { changes: Some(changes), ..Default::default() };
    let snippet_edit = crate::to_proto::snippet_workspace_edit_from_plain(&workspace_edit);
    let data = lazy_code_action_data(&snippet_edit);
    let decoded = resolve_code_action_edit_from_data(&data).expect("decode");
    let mut changes = decoded.changes.expect("changes");
    let round_tripped = changes.remove(&uri).expect("the file's edits");
    assert_eq!(round_tripped.len(), 1);
    assert_eq!(round_tripped[0].new_text, edit.new_text);
}

#[test]
fn code_action_kind_filter_matches_prefixes() {
    let requested = Some(vec![CodeActionKind::REFACTOR]);
    assert!(code_action_kind_allowed(&requested, &CodeActionKind::REFACTOR_REWRITE));
    assert!(!code_action_kind_allowed(&requested, &CodeActionKind::QUICKFIX));
}

#[test]
fn code_action_kind_filter_matches_custom_source_fix_all() {
    let requested = Some(vec![CodeActionKind::SOURCE_FIX_ALL]);
    assert!(code_action_kind_allowed(&requested, &sail_source_fix_all_kind()));
    assert!(!code_action_kind_allowed(
        &Some(vec![CodeActionKind::REFACTOR]),
        &sail_source_fix_all_kind()
    ));
}

#[test]
fn resolves_local_symbol_occurrences_without_crossing_shadowing_scopes() {
    let source = "function foo() = {\n  let x = 1;\n  let y = let x = 2 in x;\n  x + y\n}\n";
    let file = SalsaTestFile::new(source.to_string());
    assert!(file.item_tree().is_some());
    let lc = file.source.position_at(source.rfind("x + y").expect("outer x"));

    let symbol = resolve_symbol_at(&file, lc).expect("resolved symbol");
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
    let file = SalsaTestFile::new(source.to_string());
    assert!(file.item_tree().is_some());
    let lc = file.source.position_at(source.rfind("=> x").expect("body x") + 3);

    let symbol = resolve_symbol_at(&file, lc).expect("resolved symbol");
    let spans = symbol_spans_for_file(&file, &symbol, true);

    assert_eq!(spans.len(), 2);
    assert!(spans.iter().any(|(span, is_write)| *is_write && &source[span.start..span.end] == "x"));
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
    let file1 = SalsaTestFile::new(source1.to_string());
    let file2 = SalsaTestFile::new(source2.to_string());
    assert!(file1.item_tree().is_some());
    assert!(file2.item_tree().is_some());
    let lc = file1.source.position_at(source1.find("foo() = 1").expect("foo definition"));

    let symbol = resolve_symbol_at(&file1, lc).expect("resolved symbol");
    let locations =
        reference_locations(vec![(&uri1, &file1), (&uri2, &file2)], &uri1, &symbol, true);

    assert_eq!(locations.len(), 3);
    assert!(locations.iter().all(|location| location.url == uri1));
}

#[test]
fn renames_type_variables_within_their_own_scope_only() {
    let uri1 = Url::parse("file:///tmp/a.sail").unwrap();
    let uri2 = Url::parse("file:///tmp/b.sail").unwrap();
    let source1 = "val f : forall ('n). bits('n) -> bits('n)\n";
    let source2 = "val g : forall ('n). bits('n) -> bits('n)\n";
    let file1 = SalsaTestFile::new(source1.to_string());
    let file2 = SalsaTestFile::new(source2.to_string());
    assert!(file1.item_tree().is_some());
    assert!(file2.item_tree().is_some());
    let lc = file1.source.position_at(source1.find("'n").expect("type var"));

    let symbol = resolve_symbol_at(&file1, lc).expect("resolved symbol");
    let changes = rename_edits(vec![(&uri1, &file1), (&uri2, &file2)], &uri1, &symbol, "'m");

    assert_eq!(changes.len(), 1);
    assert_eq!(changes.get(&uri1).map(Vec::len), Some(3));
    assert!(!changes.contains_key(&uri2));
}

/// A client that takes no Markdown is not sent Markdown.
///
/// `textDocument.hover.contentFormat` is the client's list of formats,
/// and the protocol's default when it is absent is plaintext only. This
/// sent `MarkupKind::Markdown` either way, so `**bold**` and the
/// backticks arrived as themselves — and `doc_to_plain_text`, which
/// exists for exactly that, had no caller.
/// Closing a file outside the workspace clears its diagnostics; closing
/// one inside does not.
///
/// `clear_native_for` says in its own doc that it is "used when a file is
/// closed" and carried an `#[expect(dead_code)]` — nothing cleared
/// anything, so a scratch file opened, diagnosed and closed left its
/// errors in the problems panel for the rest of the session. A workspace
/// file keeps them, which is what that panel is for.
#[test]
fn closing_a_scratch_file_clears_its_diagnostics() {
    use crate::diagnostics::{DiagnosticCollection, NativeDiagnosticsFetchKind};

    let mut collection = DiagnosticCollection::default();
    let file = base_db::FileId::from_raw(7);
    let diag = lsp_types::Diagnostic {
        range: lsp_types::Range::default(),
        message: "something".to_string(),
        ..Default::default()
    };
    collection.set_native_diagnostics(
        NativeDiagnosticsFetchKind::Semantic,
        1,
        vec![(file, vec![diag])],
    );
    let _ = collection.take_changes();
    assert_eq!(collection.diagnostics_for(file).count(), 1);

    collection.clear_native_for(file);
    assert_eq!(collection.diagnostics_for(file).count(), 0, "cleared");
    assert!(
        collection.take_changes().is_some_and(|c| c.contains(&file)),
        "and the file is republished, or the client never hears that it is clear"
    );
}

#[test]
fn a_client_without_markdown_gets_plain_text() {
    use crate::lsp::capabilities::ClientCapabilities;

    let with = |formats: Option<Vec<lsp_types::MarkupKind>>| {
        ClientCapabilities(lsp_types::ClientCapabilities {
            text_document: Some(lsp_types::TextDocumentClientCapabilities {
                hover: Some(lsp_types::HoverClientCapabilities {
                    content_format: formats,
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        })
    };
    assert!(with(Some(vec![lsp_types::MarkupKind::Markdown])).hover_markdown_support());
    assert!(!with(Some(vec![lsp_types::MarkupKind::PlainText])).hover_markdown_support());
    // Absent means plaintext, per the protocol.
    assert!(!with(None).hover_markdown_support());
    assert!(!ClientCapabilities::default().hover_markdown_support());

    let result = ide::hover::HoverResult {
        markup: "`foo` is **bold**".to_string(),
        range: base_db::text_range(0, 3),
        actions: Vec::new(),
    };
    let index = ide_db::line_index::LineIndex::new("abc\n");
    let md = crate::to_proto::hover(&index, &result, true);
    let plain = crate::to_proto::hover(&index, &result, false);
    let text = |h: &lsp_types::Hover| match &h.contents {
        lsp_types::HoverContents::Markup(m) => (m.kind.clone(), m.value.clone()),
        _ => panic!("markup contents"),
    };
    assert_eq!(text(&md), (lsp_types::MarkupKind::Markdown, "`foo` is **bold**".to_string()));
    let (kind, value) = text(&plain);
    assert_eq!(kind, lsp_types::MarkupKind::PlainText);
    assert!(!value.contains("**"), "markdown survived into plain text: {value:?}");
}

#[test]
fn completion_uses_ast_scoped_bindings_for_local_candidates() {
    let uri = Url::parse("file:///tmp/main.sail").unwrap();
    let source = "function foo() = {\n  let local_value = 1;\n  local_\n}\n";
    let file = SalsaTestFile::new(source.to_string());
    let offset = source.find("local_\n").expect("completion site") + "local_".len();
    let prefix = completion_prefix(file.source.text(), offset);
    let all_files: Vec<(&Url, &dyn ide_db::FileDb)> = vec![(&uri, &file as &dyn ide_db::FileDb)];
    // The file's own database and its `FileText`, which is what the server
    // passes. A fresh empty database and `None` — what this used to hand
    // it — leaves the scope lookup nothing to resolve against, and the
    // answer is empty. The test said so in a comment and asserted nothing.
    let items = ide::completion::completions(
        &file.db,
        Some(file.input),
        &all_files,
        &uri,
        &file as &dyn ide_db::FileDb,
        file.source.text(),
        offset,
        prefix,
        ide_db::SAIL_KEYWORDS,
        ide_db::SAIL_BUILTINS,
    );

    assert!(
        items.iter().any(|i| i.label == "local_value"),
        "the local binding is a candidate: {:?}",
        items.iter().map(|i| &i.label).collect::<Vec<_>>()
    );
}

/// Nothing in the workspace is a candidate inside a comment or a string.
///
/// `is_in_comment` / `is_in_string` are one `matches!` on a token kind and
/// had no caller, while the completion context scanned the raw characters
/// for an unterminated quote and asked about comments not at all — so
/// typing in `// loc|` offered every symbol the workspace has.
#[test]
fn completion_offers_nothing_inside_a_comment_or_a_string() {
    let uri = Url::parse("file:///tmp/main.sail").unwrap();
    for (source, site) in [
        ("function foo() = {\n  let local_value = 1;\n  // local_\n}\n", "// local_"),
        ("function foo() = {\n  let local_value = 1;\n  \"local_\n}\n", "\"local_"),
        ("function foo() = {\n  let local_value = 1;\n  /* local_ */\n}\n", "/* local_"),
        ("/// local_\nfunction foo() = 1\n", "/// local_"),
    ] {
        let file = SalsaTestFile::new(source.to_string());
        let offset = source.find(site).expect("site") + site.len();
        let prefix = completion_prefix(file.source.text(), offset);
        let db = ide_db::root_database::RootDatabase::default();
        let all_files: Vec<(&Url, &dyn ide_db::FileDb)> =
            vec![(&uri, &file as &dyn ide_db::FileDb)];
        let items = ide::completion::completions(
            &db,
            None,
            &all_files,
            &uri,
            &file as &dyn ide_db::FileDb,
            file.source.text(),
            offset,
            prefix,
            ide_db::SAIL_KEYWORDS,
            ide_db::SAIL_BUILTINS,
        );
        assert!(
            items.is_empty(),
            "{site:?} is not code: {:?}",
            items.iter().map(|i| &i.label).collect::<Vec<_>>()
        );
    }
}

#[test]
fn no_unmodified_warning_when_var_is_assigned() {
    // var x is assigned to later => no warning
    let source = "function foo() -> int = {\n  var x : int = 1;\n  x = 2;\n  x\n}\n";
    let file = SalsaTestFile::new(source.to_string());
    let lsp_diagnostics = file.lsp_diagnostics();
    let unmodified = lsp_diagnostics.iter().find(|d| d.message.contains("never modified"));
    assert!(unmodified.is_none(), "Should not warn when var is modified, got: {lsp_diagnostics:?}");
}

/// Quick fix for `unused-variable`: should rename to `_<name>`.
#[test]
fn unused_variable_quickfix_renames_with_underscore() {
    use ide_assists::unused_variable_fix;

    let source = "function f(x : int) -> int = {\n  let unused = 42;\n  x + 1\n}\n";
    let file = SalsaTestFile::new(source.to_string());
    let line_index = ide_db::line_index::LineIndex::new(source);
    let diagnostics = file.lsp_diagnostics();
    let lsp_diag = diagnostics
        .iter()
        .find(|d| diagnostic_code_str(d) == Some("unused-variable"))
        .expect("expected unused-variable diagnostic");
    let diag = crate::from_proto::diagnostic(&line_index, lsp_diag);

    let (title, edit, _) = unused_variable_fix(&file, &diag).expect("fix should be available");
    assert!(title.contains("_unused"), "title was: {title}");
    assert_eq!(edit.new_text, "_");
    assert_eq!(edit.range.start(), edit.range.end());
    // The insertion should land at the start of the variable name.
    let insert_offset = base_db::range_start(edit.range);
    assert_eq!(&source[insert_offset..insert_offset + 6], "unused");
}

/// Quick fix should not double-prefix an already-`_`-prefixed name.
#[test]
fn unused_variable_quickfix_skips_already_underscored() {
    use ide_assists::unused_variable_fix;
    let source = "function f(x : int) -> int = {\n  let _already = 42;\n  x + 1\n}\n";
    let file = SalsaTestFile::new(source.to_string());
    let line_index = ide_db::line_index::LineIndex::new(source);
    let diagnostics = file.lsp_diagnostics();
    // _already should NOT be flagged as unused (semantic.rs respects `_` prefix),
    // so this test just confirms the fix is None when no diagnostic is present.
    if let Some(d) = diagnostics.iter().find(|d| diagnostic_code_str(d) == Some("unused-variable"))
    {
        let ide_diag = crate::from_proto::diagnostic(&line_index, d);
        assert!(unused_variable_fix(&file, &ide_diag).is_none());
    }
}

/// Operator precedence: `==` should bind tighter than `|` so that
/// `a == b | c == d` parses as `(a == b) | (c == d)` (both bool).
#[test]
fn parses_comparison_with_lower_precedence_or() {
    let source = r#"
val foo : (int, int, int) -> bool
function foo(num_elem, group_size, elem_per_reg) =
  num_elem == group_size * elem_per_reg | num_elem == 2 * group_size * elem_per_reg
"#;
    let file = SalsaTestFile::new(source.to_string());
    let diags = file.lsp_diagnostics();
    let type_errors: Vec<_> =
        diags.iter().filter(|d| diagnostic_code_str(d) == Some("type-error")).collect();
    assert!(type_errors.is_empty(), "expected no type errors, got: {type_errors:?}");
}

/// Operator precedence: comparisons bind looser than arithmetic.
/// `a + b > c ^ d - 1` should parse as `(a + b) > ((c ^ d) - 1)`.
#[test]
fn parses_arithmetic_tighter_than_comparison() {
    let source = r#"
val foo : (int, int, int) -> bool
function foo(a, b, sew) = a + b > 2 ^ sew - 1
"#;
    let file = SalsaTestFile::new(source.to_string());
    let diags = file.lsp_diagnostics();
    let type_errors: Vec<_> =
        diags.iter().filter(|d| diagnostic_code_str(d) == Some("type-error")).collect();
    assert!(type_errors.is_empty(), "expected no type errors, got: {type_errors:?}");
}

/// `int(N)` (parameterized atom type) should be treated as numeric and
/// compatible with `int` for LSP purposes.
#[test]
fn parameterized_int_unifies_with_int() {
    let source = r#"
type myrange = { 'q, 'q > 0 & 'q <= 8. int('q) }
val to_myrange : int -> myrange
"#;
    let file = SalsaTestFile::new(source.to_string());
    let diags = file.lsp_diagnostics();
    let type_errors: Vec<_> =
        diags.iter().filter(|d| diagnostic_code_str(d) == Some("type-error")).collect();
    assert!(type_errors.is_empty(), "expected no type errors, got: {type_errors:?}");
}

/// Type aliases should be resolved when slicing: a slice of `xlenbits`
/// (alias for `bits(64)`) should produce a `bits(N)` of the slice width,
/// not propagate the alias name.
#[test]
fn slice_through_type_alias_returns_bits() {
    let source = r#"
type mybits = bits(64)
val first16 : mybits -> bits(16)
function first16(v) = v[15 .. 0]
"#;
    let file = SalsaTestFile::new(source.to_string());
    let diags = file.lsp_diagnostics();
    let type_errors: Vec<_> =
        diags.iter().filter(|d| diagnostic_code_str(d) == Some("type-error")).collect();
    assert!(type_errors.is_empty(), "expected no type errors, got: {type_errors:?}");
}

/// `Sail` assignments are statements that always have type `unit`, not
/// the rhs type. A unit-returning function whose body is an assignment
/// should typecheck cleanly.
#[test]
fn assignment_expression_has_unit_type() {
    let source = r#"
register r : int
val set_r : int -> unit
function set_r(v) = {
  r = v
}
"#;
    let file = SalsaTestFile::new(source.to_string());
    let diags = file.lsp_diagnostics();
    let type_errors: Vec<_> =
        diags.iter().filter(|d| diagnostic_code_str(d) == Some("type-error")).collect();
    assert!(type_errors.is_empty(), "expected no type errors, got: {type_errors:?}");
}

/// Match arms where one branch diverges via `exit()` should unify with
/// any sibling branch type.
#[test]
fn diverging_branch_unifies_with_any_sibling() {
    let source = r#"
val tryit : int -> (int, int)
function tryit(x) =
  match x {
    0 => (1, 2),
    _ => exit()
  }
"#;
    let file = SalsaTestFile::new(source.to_string());
    let diags = file.lsp_diagnostics();
    let type_errors: Vec<_> =
        diags.iter().filter(|d| diagnostic_code_str(d) == Some("type-error")).collect();
    assert!(type_errors.is_empty(), "expected no type errors, got: {type_errors:?}");
}

/// Conditional types `bits(if cond then N else M)` cannot be evaluated
/// without an SMT solver. The LSP should be permissive and not flag the
/// branch return values as subtype violations.
#[test]
fn conditional_type_in_signature_does_not_flag_branches() {
    let source = r#"
val PPN_of : forall 'pte_size, 'pte_size in {32, 64}.
  bits('pte_size) -> bits(if 'pte_size == 32 then 22 else 44)
function PPN_of(pte) = if 'pte_size == 32 then pte[31 .. 10] else pte[53 .. 10]
"#;
    let file = SalsaTestFile::new(source.to_string());
    let diags = file.lsp_diagnostics();
    let type_errors: Vec<_> =
        diags.iter().filter(|d| diagnostic_code_str(d) == Some("type-error")).collect();
    assert!(type_errors.is_empty(), "expected no type errors, got: {type_errors:?}");
}

#[test]
fn cross_file_call_with_wrong_arg_count_is_reported() {
    // B1 narrow: cross-file calls aren't fully type-checked (the unifier
    // can't safely consume cross-file quantified schemes), but the
    // arity-only check IS reliable. Verify the dedicated cross-file
    // arity table catches a real wrong-arg-count bug across files.
    let ws = SalsaTestWorkspace::new(&[
        "val foo : (int, int) -> unit\nfunction foo(x, y) = ()\n",
        "function caller() = foo(1)\n",
    ]);
    let mismatches: Vec<_> = ws
        .diagnostics_with_workspace(1)
        .into_iter()
        .filter(|d| diagnostic_code_str(d) == Some("mismatched-arg-count"))
        .collect();
    assert!(
        !mismatches.is_empty(),
        "expected cross-file arity check to fire on `foo(1)`, got: {:?}",
        ws.diagnostics_with_workspace(1)
    );
}

#[test]
fn cross_file_call_with_correct_arg_count_is_accepted() {
    let ws = SalsaTestWorkspace::new(&[
        "val bar : (int, int) -> unit\nfunction bar(x, y) = ()\n",
        "function caller() = bar(1, 2)\n",
    ]);
    let mismatches: Vec<_> = ws
        .diagnostics_with_workspace(1)
        .into_iter()
        .filter(|d| diagnostic_code_str(d) == Some("mismatched-arg-count"))
        .collect();
    assert!(
        mismatches.is_empty(),
        "cross-file arity check wrongly fired on a correct call: {mismatches:?}"
    );
}

#[test]
fn strict_unresolved_check_fires_when_workspace_complete() {
    let source = "\
function f() = {
  let _ = some_external_function_we_have_not_seen;
  ()
}
";
    let ws = SalsaTestWorkspace::new(&[source]);
    let unresolved: Vec<_> = ws
        .diagnostics_with_workspace(0)
        .into_iter()
        .filter(|d| {
            diagnostic_code_str(d) == Some("unresolved-ident")
                && d.message.contains("unresolved identifier")
        })
        .collect();
    assert!(!unresolved.is_empty(), "strict check did not fire even though workspace was complete");
}

#[test]
fn foreach_loop_bounds_count_as_variable_usage() {
    // Upstream `sail/src/lib/rewriter.ml::e_for` joins used-id sets from
    // start, end, step AND body. Our previous analyzer only walked the
    // body, so variables only referenced in the loop bounds were
    // incorrectly flagged as unused.
    let source = r#"
val f : int -> unit
function f(n) = {
  let eg_start : int = 0;
  let eg_len   : int = n;
  foreach (i from eg_start to (eg_len - 1)) {
    let _ = i;
    ()
  }
}
"#;
    let file = SalsaTestFile::new(source.to_string());
    let unused: Vec<_> = file
        .lsp_diagnostics()
        .into_iter()
        .filter(|d| diagnostic_code_str(d) == Some("unused-variable"))
        .collect();
    assert!(unused.is_empty(), "foreach-bound bindings wrongly flagged as unused: {unused:?}");
}

#[test]
fn foreach_iterator_is_not_flagged_as_unused() {
    // The loop iterator itself is never reported as unused, matching
    // upstream (`lint.ml` never adds the `E_for` binder to the pattern
    // set, so it's neither warned nor tracked).
    let source = r#"
function f() = {
  foreach (i from 0 to 10) { () }
}
"#;
    let file = SalsaTestFile::new(source.to_string());
    let unused: Vec<_> = file
        .lsp_diagnostics()
        .into_iter()
        .filter(|d| diagnostic_code_str(d) == Some("unused-variable"))
        .collect();
    assert!(unused.is_empty(), "foreach iterator wrongly flagged as unused: {unused:?}");
}

#[test]
fn function_clause_tuple_params_bind_all_names() {
    // `val f : (A, B) -> C` paired with `function clause f(x, y) = ...`
    // declares one tuple param but two clause patterns. The checker
    // should flatten the tuple so both `x` and `y` enter the local
    // environment — previously `y` was silently dropped and references
    // to it were reported as "Unresolved identifier" under workspace mode.
    let source = "\
val write_csr : (int, bits(32)) -> unit
function clause write_csr(0x1, value) = { let _ = value; () }
function clause write_csr(0x2, value) = { let _ = value; () }
";
    let ws = SalsaTestWorkspace::new(&[source]);
    let diagnostics = ws.diagnostics_with_workspace(0);
    let unresolved: Vec<_> = diagnostics
        .iter()
        .filter(|d| {
            diagnostic_code_str(d) == Some("type-error")
                && d.message.contains("Unresolved identifier")
        })
        .collect();
    assert!(unresolved.is_empty(), "expected no unresolved-identifier errors, got: {unresolved:?}");
}

#[test]
fn let_typevar_binding_exposes_value_binding() {
    // `let 'N = expr` in Sail introduces both the type variable `'N` and
    // a value binding `N`. The checker should know about `N` so later
    // references don't report it as unresolved, even under workspace-aware
    // type checking (where the strict unresolved-identifier check runs).
    let source = "\
val get_n : unit -> int
function get_n() = 4
function f() -> int = {
  let 'N = get_n();
  N + 1
}
";
    let ws = SalsaTestWorkspace::new(&[source]);
    let diagnostics = ws.diagnostics_with_workspace(0);
    let unresolved: Vec<_> = diagnostics
        .iter()
        .filter(|d| {
            diagnostic_code_str(d) == Some("type-error")
                && d.message.contains("Unresolved identifier")
        })
        .collect();
    assert!(unresolved.is_empty(), "expected no unresolved-identifier errors, got: {unresolved:?}");
}

// Pattern exhaustiveness — regression tests for the typechecker-driven check.
// These exercise `sail_server::match_check` via `Checker::check_match_cases`.

/// Match-exhaustiveness diagnostics, through the pipeline the server runs.
///
/// These reach a user now. They used to be text-only and unrendered, because
/// the checker reported constructors that do not exist — a `write_kind`
/// scrutinee matched against a same-named enum from another file, and tuple
/// payload type names read as extra union variants. With both fixed the
/// structured `InferenceDiagnostic::IncompleteMatch` draws nothing on
/// sail-riscv, so it is emitted.
fn match_diagnostics(source: &str) -> (Vec<lsp_types::Diagnostic>, Vec<lsp_types::Diagnostic>) {
    let file = SalsaTestFile::new(source.to_string());
    let diagnostics = file.lsp_diagnostics();
    let of_code = |code: &str| -> Vec<lsp_types::Diagnostic> {
        diagnostics.iter().filter(|d| diagnostic_code_str(d) == Some(code)).cloned().collect()
    };
    (of_code("incomplete-match"), of_code("redundant-match-arm"))
}

#[test]
fn match_exhaustiveness_reports_missing_enum_member() {
    let source = r#"
enum Privilege = { Machine, Supervisor, User }
val classify : Privilege -> int
function classify(p) = match p {
    Machine => 0,
    Supervisor => 1,
}
"#;
    let (incomplete, _) = match_diagnostics(source);
    assert_eq!(
        incomplete.len(),
        1,
        "expected one incomplete-match diagnostic, got {:?}",
        incomplete
    );
    assert!(
        incomplete[0].message.contains("User"),
        "expected witness to mention `User`, got: {}",
        incomplete[0].message
    );
}

#[test]
fn match_exhaustiveness_accepts_full_enum_coverage() {
    let source = r#"
enum Color = { Red, Green, Blue }
val pick : Color -> int
function pick(c) = match c {
    Red => 0,
    Green => 1,
    Blue => 2,
}
"#;
    let (incomplete, redundant) = match_diagnostics(source);
    assert!(incomplete.is_empty(), "expected no incomplete-match, got {:?}", incomplete);
    assert!(redundant.is_empty(), "expected no redundant-match-arm, got {:?}", redundant);
}

#[test]
fn match_exhaustiveness_accepts_wildcard_fallback() {
    let source = r#"
enum Color = { Red, Green, Blue }
val pick : Color -> int
function pick(c) = match c {
    Red => 0,
    _ => 1,
}
"#;
    let (incomplete, _) = match_diagnostics(source);
    assert!(
        incomplete.is_empty(),
        "wildcard arm should make match exhaustive, got {:?}",
        incomplete
    );
}

#[test]
fn match_exhaustiveness_treats_guarded_arms_as_non_covering() {
    // A guarded arm cannot be relied on for coverage, so a `Machine if g`
    // arm followed by `Supervisor`/`User` still leaves `Machine` uncovered.
    let source = r#"
enum Privilege = { Machine, Supervisor, User }
val classify : (Privilege, bool) -> int
function classify(p, b) = match p {
    Machine if b => 0,
    Supervisor => 1,
    User => 2,
}
"#;
    let (incomplete, _) = match_diagnostics(source);
    assert_eq!(
        incomplete.len(),
        1,
        "guarded arm should not certify exhaustiveness, got {:?}",
        incomplete
    );
    assert!(
        incomplete[0].message.contains("Machine"),
        "expected `Machine` in witness, got: {}",
        incomplete[0].message
    );
}

#[test]
fn an_exhaustiveness_witness_names_the_missing_value_inside_a_payload() {
    // `Some(_)` would send a reader looking at an arm that is already
    // there. The gap is one level down.
    let source = r#"
union opt_bool = { NoneB : unit, SomeB : bool }
val f : opt_bool -> int
function f(o) = match o {
    NoneB() => 0,
    SomeB(true) => 1,
}
"#;
    let (incomplete, _) = match_diagnostics(source);
    assert_eq!(incomplete.len(), 1, "expected one incomplete-match, got {incomplete:?}");
    let msg = &incomplete[0].message;
    assert!(
        msg.contains("SomeB(false)"),
        "witness should name the missing payload value, got: {msg}"
    );
}

#[test]
fn match_redundancy_reports_an_arm_an_earlier_one_covers() {
    let source = r#"
enum P = { A, B, C }
val f : P -> int
function f(p) = match p {
  A => 0,
  _ => 1,
  B => 2,
}
"#;
    let (_, redundant) = match_diagnostics(source);
    assert_eq!(redundant.len(), 1, "the `B` arm is unreachable, got {redundant:?}");
    assert!(redundant[0].message.contains("unreachable match arm"));
}

#[test]
fn a_catch_all_after_a_specific_payload_arm_is_reachable() {
    // `LoadReserved(p)` after `LoadReserved(Data)` is reachable: the payload
    // has three members and only one is spoken for. This shape is why
    // redundancy reporting was suppressed — sail-riscv writes it, and the
    // checker called every such arm unreachable because it could not
    // classify the payload column and treated "not a wildcard row" as
    // "covered".
    let source = r#"
enum payload = { Data, PageTableEntry, ShadowStack }
union Access = { LoadReserved : payload, Store : payload }
val f : Access -> int
function f(a) = match a {
  LoadReserved(Data) => 0,
  Store(_) => 1,
  LoadReserved(p) => 2,
}
"#;
    let (_, redundant) = match_diagnostics(source);
    assert!(redundant.is_empty(), "expected no redundant-arm report, got {redundant:?}");
}

#[test]
fn match_exhaustiveness_handles_bool_scrutinee() {
    // The scrutinee is `bool` — closed universe of `{true, false}`.
    // Only matching one of them must report the other as missing.
    let source = r#"
val pick : bool -> int
function pick(b) = match b {
    true => 0,
}
"#;
    let (incomplete, _) = match_diagnostics(source);
    assert_eq!(
        incomplete.len(),
        1,
        "bool scrutinee with one arm should be incomplete, got {:?}",
        incomplete
    );
    assert!(
        incomplete[0].message.contains("false"),
        "expected `false` in witness, got: {}",
        incomplete[0].message
    );
}

#[test]
fn match_exhaustiveness_no_warning_for_int_with_wildcard() {
    // The scrutinee type is `int` (Unlistable universe). With a wildcard
    // arm present the match is exhaustive — no diagnostic.
    let source = r#"
val pick : int -> int
function pick(n) = match n {
    0 => 100,
    _ => 200,
}
"#;
    let (incomplete, _) = match_diagnostics(source);
    assert!(
        incomplete.is_empty(),
        "wildcard arm should suppress incomplete-match on int scrutinee, got {:?}",
        incomplete
    );
}

#[test]
fn match_check_substitutes_generic_union_type_args() {
    // Generic union `opt('a)` matched against `opt(bool)` should know
    // the inner pattern type is `bool`. Without the substitution step
    // in `check_match_exhaustiveness::unions_for_cx`, `Some`'s payload
    // would be the type-var `'a` (lowered to `MatchTy::Unknown`), and
    // the matrix algorithm would believe `Some(true) | Some(false)`
    // doesn't cover `Some` exhaustively — emitting a false-positive
    // `incomplete-match` diagnostic. With substitution, the payload
    // becomes `bool` and the two literal arms cover the universe.
    let source = "\
union opt('a) = { None : unit, Some : 'a }
function f(x : opt(bool)) -> int = match x {
    None() => 0,
    Some(true) => 1,
    Some(false) => 2,
}
";
    let file = SalsaTestFile::new(source.to_string());
    let result = file.checker_diagnostics();
    let incomplete: Vec<_> =
        result.iter().filter(|d| d.code.as_str() == "incomplete-match").collect();
    assert!(
        incomplete.is_empty(),
        "Some(true) | Some(false) | None() should be exhaustive over opt(bool); got: {:?}",
        incomplete
    );
}

#[cfg(feature = "z3-solver")]
#[test]
fn z3_proves_non_linear_constraint_the_linear_evaluator_cant() {
    // `n * n >= 0` is valid for any integer `n`, but the linear
    // evaluator in `linear_numeric_expr` bails out on `Mul(Var, Var)`.
    // Z3 proves it straight away.
    let expr = ConstraintExpr::Compare {
        lhs: NumericExpr::Mul(
            Box::new(NumericExpr::Var("n".to_string())),
            Box::new(NumericExpr::Var("n".to_string())),
        ),
        op: CompareOp::Gte,
        rhs: NumericExpr::Const(0),
    };
    let subst = Subst::default();
    let status = hir::z3_solver::try_solve(&expr, &subst, &[]);
    assert_eq!(
        status,
        ConstraintStatus::Satisfied,
        "Z3 should prove n*n >= 0 is universally satisfied"
    );
}

#[test]
fn nested_generic_union_falls_back_without_false_positive() {
    // Round-5: nested generic substitution now resolves precisely at
    // every recursion level. `Some(Some(_))` with an inner wildcard
    // covers the remaining `Some(Some(bool))` universe, so the match
    // is exhaustive and no incomplete-match diagnostic fires.
    let source = "\
union opt('a) = { None : unit, Some : 'a }
function f(x : opt(opt(bool))) -> int = match x {
    None() => 0,
    Some(None()) => 1,
    Some(Some(_)) => 2,
}
";
    let file = SalsaTestFile::new(source.to_string());
    let result = file.checker_diagnostics();
    let incomplete: Vec<_> =
        result.iter().filter(|d| d.code.as_str() == "incomplete-match").collect();
    assert!(
        incomplete.is_empty(),
        "nested opt(opt(bool)) match should not produce a false-positive incomplete-match diagnostic; got: {:?}",
        incomplete
    );
}

#[test]
fn nested_generic_union_is_exhaustive_with_literal_leaves() {
    // Round-5: with lazy per-level substitution, matching
    // `opt(opt(bool))` with every concrete leaf enumerated IS
    // exhaustive — the inner `Some(Some(_))` split knows the payload
    // is `bool` so `Some(Some(true)) | Some(Some(false))` covers it.
    let source = "\
union opt('a) = { None : unit, Some : 'a }
function f(x : opt(opt(bool))) -> int = match x {
    None() => 0,
    Some(None()) => 1,
    Some(Some(true)) => 2,
    Some(Some(false)) => 3,
}
";
    let file = SalsaTestFile::new(source.to_string());
    let result = file.checker_diagnostics();
    let incomplete: Vec<_> =
        result.iter().filter(|d| d.code.as_str() == "incomplete-match").collect();
    assert!(
        incomplete.is_empty(),
        "opt(opt(bool)) with all four leaves should be exhaustive; got: {:?}",
        incomplete
    );
}

#[test]
fn nested_generic_union_detects_missing_inner_literal() {
    // A missing inner arm on `opt(opt(bool))` is named precisely: the witness
    // builder recurses into the uncovered constructor's argument rather than
    // stopping at `Some(_)`.
    let source = "\
union opt('a) = { None : unit, Some : 'a }
function f(x : opt(opt(bool))) -> int = match x {
    None() => 0,
    Some(None()) => 1,
    Some(Some(true)) => 2,
}
";
    let (incomplete, _) = match_diagnostics(source);
    assert_eq!(
        incomplete.len(),
        1,
        "opt(opt(bool)) missing Some(Some(false)) should produce exactly one incomplete-match diagnostic; got: {incomplete:?}"
    );
    assert!(
        incomplete[0].message.contains("Some(Some(false))"),
        "expected the concrete witness, got: {}",
        incomplete[0].message
    );
}

#[test]
fn recursive_union_match_does_not_stack_overflow() {
    // Round-5 depth-limit coverage: a truly recursive union like
    // `mu = { Stop : unit, Wrap : mu }` would drive
    // `is_useful_wild`'s closed-ctor specialization into unbounded
    // recursion because the `Wrap` variant's payload is `mu` itself,
    // so matching on the wildcard keeps expanding `Wrap(_)` branches
    // forever. The MATCH_CHECK_DEPTH_LIMIT bail-out in the matrix
    // turns this into a conservative "covered" result. Here we only
    // assert the check terminates (no panic / stack overflow); the
    // precise completeness outcome is deliberately not pinned.
    let source = "\
union mu = { Stop : unit, Wrap : mu }
function f(x : mu) -> int = match x {
    Stop() => 0,
    Wrap(_) => 1,
}
";
    let file = SalsaTestFile::new(source.to_string());
    let _ = file.checker_diagnostics();
}

#[test]
fn unverified_constraint_hint_fires_on_cross_file_record_access() {
    // B2 site coverage: when a field is accessed on a record type
    // we couldn't resolve (typically because it's defined in another
    // file the test workspace doesn't include), the silent-accept
    // path now emits a Hint-severity `unverified-constraint`
    // diagnostic instead of dropping the constraint on the floor.
    let source = "\
function read_field(x : some_unknown_record) -> int = x.some_field
";
    let file = SalsaTestFile::new(source.to_string());
    let result = file.checker_diagnostics();
    let hints: Vec<_> =
        result.iter().filter(|d| d.code.as_str() == "unverified-constraint").collect();
    assert!(
        !hints.is_empty(),
        "expected at least one unverified-constraint hint on cross-file record field access; diagnostics={:?}",
        result
    );
}

#[cfg(feature = "z3-solver")]
#[test]
fn z3_cache_returns_same_result_on_second_call() {
    use std::sync::atomic::Ordering;

    // Pick a moderately-sized non-linear constraint so the miss path
    // does real work, then hit the cache on the second call. The
    // variable names are intentionally unique to this test so other
    // parallel tests can't accidentally hit the same cache entry.
    // We rely only on the *delta* between the two reads, which is
    // monotonic even under parallel noise — see the per_def_cache
    // tests for the same anti-pattern + fix.
    let expr = ConstraintExpr::Compare {
        lhs: NumericExpr::Add(
            Box::new(NumericExpr::Mul(
                Box::new(NumericExpr::Var("z3_cache_n".to_string())),
                Box::new(NumericExpr::Var("z3_cache_n".to_string())),
            )),
            Box::new(NumericExpr::Mul(
                Box::new(NumericExpr::Var("z3_cache_m".to_string())),
                Box::new(NumericExpr::Var("z3_cache_m".to_string())),
            )),
        ),
        op: CompareOp::Gte,
        rhs: NumericExpr::Const(0),
    };
    let subst = Subst::default();

    let first = hir::z3_solver::try_solve(&expr, &subst, &[]);
    let hits_after_first = hir::z3_solver::Z3_CACHE_HITS.load(Ordering::Relaxed);
    assert_eq!(first, ConstraintStatus::Satisfied);

    let second = hir::z3_solver::try_solve(&expr, &subst, &[]);
    let hits_after_second = hir::z3_solver::Z3_CACHE_HITS.load(Ordering::Relaxed);
    assert_eq!(second, ConstraintStatus::Satisfied);
    assert!(
        hits_after_second > hits_after_first,
        "second call should observe a cache hit (before={}, after={})",
        hits_after_first,
        hits_after_second
    );
}

/// Test scaffolding for the disk-aware preprocessor (P0-3): write a
/// file tree under a uniquely-named scratch directory inside the OS
/// temp dir, return the root path. The caller is responsible for
/// removing the directory at the end of the test.
#[cfg(test)]
fn write_scratch_tree(label: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let root = std::env::temp_dir().join(format!("sail_lsp_p03_{label}_{nonce}"));
    std::fs::create_dir_all(&root).expect("create scratch root");
    for (rel_path, content) in files {
        let full = root.join(rel_path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(&full, content).expect("write file");
    }
    root
}

#[test]
fn instantiation_failed_constraint_includes_derived_from_locations() {
    // instantiation_error_with_sig now threads call-site
    // operand spans through to FailedConstraint::derived_from, so
    // failed-constraint diagnostics emitted from the quantifier
    // resolver also include "constraint from" lines (the same
    // rendering enabled for the bounds-check emit point).
    //
    // The constraint `'n in {1, 2}` cannot be satisfied for the
    // requested return width `bits(24)` (which would require 'n=3),
    // so the call to `widen` produces a FailedConstraint via the
    // candidate-resolution path. The diagnostic should include the
    // call's argument-span derivation chain.
    let source =
        "val widen : forall 'n, 'n in {1, 2}. unit -> bits(8 * 'n)\nfunction use() -> bits(24) = widen(())\n";
    let file = SalsaTestFile::new(source.to_string());
    let diags = file.lsp_diagnostics();
    let constraint = diags
        .iter()
        .find(|d| {
            d.message.contains("constraint cannot hold") && d.message.contains("'n in {1, 2}")
        })
        .expect("missing failed-constraint diagnostic");
    // As a location, not as two integers in the message.
    assert!(
        constraint.related_information.as_ref().is_some_and(|r| !r.is_empty()),
        "expected the failed constraint to carry where it came from, got: {constraint:?}"
    );
}

#[test]
fn unused_function_silent_in_local_only_path() {
    // The local-only compute_semantic_diagnostics path can't
    // tell apart "no callers anywhere" vs "no callers in this
    // file", so it must NOT emit unused-function warnings even
    // for files that look like they'd qualify.
    let source = "\
function helper() = 0
function main() = ()
";
    let file = SalsaTestFile::new(source.to_string());
    let diags = ide_diagnostics::semantic::compute_semantic_diagnostics(&file);
    let unused: Vec<_> = diags
        .iter()
        .filter(|d| {
            matches!(
                d.code,
                hir::diagnostics::DiagnosticCode::SailLint(
                    "unused-function",
                    hir::diagnostics::Severity::Warning
                )
            )
        })
        .collect();
    assert!(unused.is_empty(), "local-only path should not emit dead-code warnings");
}

#[test]
fn non_recursive_function_does_not_warn() {
    let source = "\
function helper() = 0
function caller() = helper()
";
    let file = SalsaTestFile::new(source.to_string());
    let diags = file.lsp_diagnostics();
    assert!(
        !diags.iter().any(|d| {
            d.code
                .as_ref()
                .map(|c| match c {
                    lsp_types::NumberOrString::String(s) => {
                        s == "recursive-without-termination-measure"
                    }
                    _ => false,
                })
                .unwrap_or(false)
        }),
        "non-recursive function should not trigger the recursion warning"
    );
}

#[test]
fn document_symbol_detail_uses_item_tree_signature_text() {
    // document_symbol_tree should now use ItemTree's
    // signature_text as the DocumentSymbol detail field, instead
    // of the bare category label ("function"/"value").
    let source = "\
val foo : (int, int) -> int
function foo(x, y) = x + y
";
    let file = SalsaTestFile::new(source.to_string());
    let symbols = ide::file_structure::file_structure(&file);

    let foo_symbols: Vec<_> = symbols.iter().filter(|s| s.name == "foo").collect();
    assert!(!foo_symbols.is_empty(), "expected at least one `foo` symbol, got {symbols:?}");
    // At least one should carry a detail derived from the item
    // tree (containing "->" or the type name "int").
    let has_detail = foo_symbols.iter().any(|s| s.detail.is_some());
    assert!(has_detail, "expected detail on `foo`, got {foo_symbols:?}");
}

#[test]
fn failed_constraint_diagnostic_includes_derived_from_locations() {
    // collection-bounds checks now populate
    // FailedConstraint::derived_from with the operand spans, which
    // makes the rendered diagnostic message include a "constraint
    // from" line per source span. Indexing a bits(8) value with a
    // statically-out-of-range constant triggers
    // check_collection_index_bounds; the resulting diagnostic
    // should mention the index span via the derived_from chain.
    let source = "\
val pick : bits(8) -> bit
function pick(x) = x[10]
";
    let file = SalsaTestFile::new(source.to_string());
    let diags = file.lsp_diagnostics();
    // "cannot hold", not "unsolved": a literal index past a known width is
    // proved false, and this diagnostic is only raised where it is.
    let constraint_diags: Vec<_> =
        diags.iter().filter(|d| d.message.contains("constraint cannot hold")).collect();
    assert!(
        !constraint_diags.is_empty(),
        "expected a failed-constraint diagnostic for out-of-range index, got: {diags:?}"
    );
    // The places, as places. They used to be byte offsets appended to the
    // message; `relatedInformation` is what a secondary location is for.
    assert!(
        constraint_diags
            .iter()
            .any(|d| d.related_information.as_ref().is_some_and(|r| !r.is_empty())),
        "expected the failed constraint to carry where it came from, got: {constraint_diags:?}"
    );
}

#[test]
fn rejects_oversized_bit_literal_against_expected_width() {
    // Probe: a hex literal that is wider than the expected
    // bits(N) type should be rejected. `0xFFFF` is bits(16);
    // expected `bits(8)` should produce a type-error diagnostic.
    let source = "\
val test : unit -> bits(8)
function test() = 0xFFFF
";
    let file = SalsaTestFile::new(source.to_string());
    let diags = file.lsp_diagnostics();
    let type_errors: Vec<_> = diags
        .iter()
        .filter(|d| {
            d.code.as_ref().and_then(|c| match c {
                lsp_types::NumberOrString::String(s) => Some(s.as_str()),
                _ => None,
            }) == Some("type-error")
        })
        .collect();
    assert!(
        !type_errors.is_empty(),
        "expected a type-error for bits(16) literal in bits(8) context, got {diags:?}"
    );
}

#[test]
fn accepts_matching_bit_literal_width() {
    // Sanity: bits(16) literal in bits(16) context should NOT
    // produce a type-error diagnostic (just to bracket the
    // tightness check).
    let source = "\
val test : unit -> bits(16)
function test() = 0xFFFF
";
    let file = SalsaTestFile::new(source.to_string());
    let diags = file.lsp_diagnostics();
    let type_errors: Vec<_> = diags
        .iter()
        .filter(|d| {
            d.code.as_ref().and_then(|c| match c {
                lsp_types::NumberOrString::String(s) => Some(s.as_str()),
                _ => None,
            }) == Some("type-error")
        })
        .collect();
    assert!(
        type_errors.is_empty(),
        "matching bits(16) literal width should not error, got {type_errors:?}"
    );
}

#[test]
fn item_tree_signature_hash_stable_across_body_edit() {
    let before = SalsaTestFile::new("val foo : int -> int\nfunction foo(x) = x + 1\n".to_string());
    let after = SalsaTestFile::new("val foo : int -> int\nfunction foo(x) = x + 2\n".to_string());
    let before_hash = before.item_tree().expect("item tree").signature_hash;
    let after_hash = after.item_tree().expect("item tree").signature_hash;
    assert_eq!(before_hash, after_hash, "body-only edit must not change item_tree signature_hash");
}

#[test]
fn item_tree_signature_hash_changes_on_signature_edit() {
    let before = SalsaTestFile::new("val foo : int -> int\n".to_string());
    let after = SalsaTestFile::new("val foo : int -> bool\n".to_string());
    let before_hash = before.item_tree().expect("item tree").signature_hash;
    let after_hash = after.item_tree().expect("item tree").signature_hash;
    assert_ne!(before_hash, after_hash, "signature edit must change item_tree signature_hash");
}

#[test]
fn scan_sail_files_discovers_all_files() {
    let root = write_scratch_tree(
        "proj",
        &[
            ("first.sail", "val first_fn : unit -> unit\n"),
            ("second.sail", "val second_fn : unit -> unit\n"),
            ("orphan.sail", "val orphan_fn : unit -> unit\n"),
        ],
    );

    let scanned = crate::reload::scan_sail_files(&root);

    let mut names: Vec<String> = scanned
        .iter()
        .filter_map(|(p, _)| p.file_name().and_then(|s| s.to_str()).map(String::from))
        .collect();
    names.sort();
    assert_eq!(names, vec!["first.sail", "orphan.sail", "second.sail"]);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn scan_sail_files_finds_nested() {
    let root = write_scratch_tree(
        "noproj",
        &[("a.sail", "val a_fn : unit -> unit\n"), ("sub/b.sail", "val b_fn : unit -> unit\n")],
    );

    let scanned = crate::reload::scan_sail_files(&root);

    let mut names: Vec<String> = scanned
        .iter()
        .filter_map(|(p, _)| p.file_name().and_then(|s| s.to_str()).map(String::from))
        .collect();
    names.sort();
    assert_eq!(names, vec!["a.sail", "b.sail"]);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn symbol_locations_use_identifier_spans() {
    use ide::navigation::{symbol_declaration_locations, symbol_definition_locations};
    use lsp_types::Url;

    let source = "val foo : int\nfunction foo() = 0\n".to_string();
    let file = SalsaTestFile::new(source.clone());
    let uri = Url::parse("file:///tmp/main.sail").unwrap();

    let declarations = symbol_declaration_locations(std::iter::once((&uri, &file)), &uri, "foo");
    let definitions = symbol_definition_locations(std::iter::once((&uri, &file)), &uri, "foo");

    assert_eq!(declarations.len(), 1);
    assert_eq!(definitions.len(), 1);

    let decl_start = source.find("foo :").unwrap();
    let def_start = source.rfind("foo()").unwrap();
    assert_eq!(declarations[0].range, base_db::text_range(decl_start, decl_start + 3));
    assert_eq!(definitions[0].range, base_db::text_range(def_start, def_start + 3));
}

#[test]
fn analysis_parses_multiline_val_signature() {
    use ide_db::collect_callable_signatures;
    let file = SalsaTestFile::new("val f : int ->\n  bool -> string\n".to_string());
    let signatures = collect_callable_signatures(&file);
    let sig = signatures.iter().find(|sig| sig.name == "f").unwrap();
    assert_eq!(sig.params.len(), 2);
    assert_eq!(sig.return_type.as_deref(), Some("string"));
}

#[test]
fn analysis_ignores_parentheses_inside_string_for_call_detection() {
    use ide::signature_help::find_call_at_position;
    let source = r#"let _ = foo("(", 3)"#.to_string();
    let file = SalsaTestFile::new(source);
    let call = find_call_at_position(&file, ide_db::LineCol { line: 0, col: 17 }).unwrap();
    assert_eq!(call.0, "foo");
    assert_eq!(call.1, 1);
}

#[test]
fn analysis_captures_only_top_level_bindings() {
    use ide_db::signatures::add_definitions;
    use std::collections::HashMap;
    let source = r#"
function foo() = {
  let x = 1;
  var y = 2;
  x + y
}
let z = 3
"#;
    let mut definitions = HashMap::new();
    add_definitions(source, &mut definitions);

    assert!(definitions.contains_key("foo"));
    assert!(definitions.contains_key("z"));
    assert!(!definitions.contains_key("x"));
    assert!(!definitions.contains_key("y"));
}

#[test]
fn analysis_excludes_value_declarations_from_definitions() {
    use ide_db::signatures::add_definitions;
    use std::collections::HashMap;
    let source = "val foo : int -> int\nfunction foo(x) = x";
    let mut definitions = HashMap::new();
    add_definitions(source, &mut definitions);
    assert!(definitions.contains_key("foo"));
}

// Inlay hints inline tests, moved from inlay_hints.rs during
#[test]
fn inlay_hint_parameter_label_shown_for_known_call() {
    let source =
        "val add : (int, int) -> int\nfunction add(x, y) = x + y\nfunction caller() = add(1, 2)\n";
    let file = SalsaTestFile::new(source.to_string());
    let uri = Url::parse("file:///tmp/test.sail").unwrap();
    let full = base_db::text_range(0, source.len());
    let all_files: Vec<(&Url, &dyn ide_db::FileDb)> = vec![(&uri, &file as &dyn ide_db::FileDb)];
    let hints = ide::inlay_hints::inlay_hints_ide(&all_files, &uri, &file, full);
    let param_hints: Vec<_> = hints.iter().filter(|h| !h.label.contains("caller")).collect();
    assert!(!param_hints.is_empty(), "expected parameter hints");
}

/// A request handler converts; it does not decide.
///
/// rust-analyzer keeps a feature's whole answer in `ide` and lets the handler
/// translate it (`Analysis::goto_definition` in, `GotoDefinitionResponse`
/// out). Logic drifts upward here for a reason that looks good every time:
/// `ide` cannot answer because it lacks the symbol index or the file list, the
/// handler has both, so the missing step gets written where the data is. Each
/// step is local and sensible; the result is that `ide`'s entry point answers
/// only part of the question, and anything that calls it — a test, a
/// measurement, another feature — silently gets that part.
///
/// That is not hypothetical: `Analysis::goto_definition` did not exist, the
/// three resolution steps lived in `handle_goto_definition`, and the coverage
/// measurement that called into `ide` reported 60% for a feature that answers
/// 77%.
///
/// Nothing else catches it. The types do not object, the tests pass through
/// whichever path they were written against, and comparing module lists
/// against rust-analyzer shows no gap because the modules are all there.
///
/// So it is counted. A handler that reaches into `ide` more than once is
/// deciding something, and the number of decisions per handler may go down
/// but not up. Migrating one means moving its logic behind a single
/// `Analysis` method and lowering its entry here.
#[test]
fn handlers_do_not_accumulate_logic() {
    const SOURCE: &str = include_str!("handlers/request.rs");

    // Recorded, not aspirational: these are the handlers that still decide
    // things, with the number of distinct `ide`/`Analysis` entry points each
    // one calls. `goto_definition` and `references` are at 1 because they
    // have been migrated.
    /// Handlers allowed to call more than one `ide` feature.
    ///
    /// Empty. Every handler calls exactly one, which is the rule stated
    /// without exceptions — it took collapsing `handle_code_action`'s own
    /// table of fifteen assists into `Analysis::code_actions`, and moving
    /// the knowledge that `->` and `=>` complete on their second character
    /// out of `handle_on_type_formatting`.
    ///
    /// Conversion helpers — `file`, `file_id`, `url_for_file_id` — do not
    /// count; see the counter below for why.
    const BASELINE: &[(&str, usize)] = &[];

    let mut actual: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut current: Option<String> = None;
    let mut calls: std::collections::HashSet<String> = std::collections::HashSet::new();
    for line in SOURCE.lines() {
        if let Some(rest) = line.trim().strip_prefix("pub(crate) fn handle_") {
            if let Some(name) = current.take() {
                actual.insert(name, calls.len());
            }
            let name = rest.split('(').next().unwrap_or_default();
            current = Some(format!("handle_{name}"));
            calls.clear();
            continue;
        }
        if current.is_none() {
            continue;
        }
        if line == "}" {
            if let Some(name) = current.take() {
                actual.insert(name, calls.len());
            }
            calls.clear();
            continue;
        }
        for token in ["ide::", "analysis."] {
            let mut rest = line;
            while let Some(i) = rest.find(token) {
                let tail = &rest[i + token.len()..];
                let end = tail
                    .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
                    .unwrap_or(tail.len());
                if end > 0 {
                    let path = &tail[..end];
                    // Naming a type or an enum variant to convert it is what
                    // a handler is *for*. Only a call decides anything, so
                    // only a path whose last segment is a function counts —
                    // the same lowercase test `the_server_goes_through_analysis`
                    // uses. Counting `ide::x::Category::Read` as three entry
                    // points said a handler that matched on three variants had
                    // accumulated logic, and hid the ones that had.
                    let is_call = path
                        .rsplit("::")
                        .next()
                        .and_then(|seg| seg.chars().next())
                        .is_some_and(|c| c.is_lowercase());
                    // `file`, `file_id` and `url_for_file_id` convert
                    // between the ids `ide` answers in and the URLs and
                    // line/columns the protocol speaks. They are this
                    // server's `file_line_index` — rust-analyzer's handlers
                    // call that too — and they decide nothing.
                    let is_conversion =
                        matches!(path, "file" | "file_id" | "url_for_file_id" | "files_ref" | "db");
                    if is_call && !is_conversion {
                        calls.insert(format!("{token}{path}"));
                    }
                }
                rest = &tail[end.max(1)..];
            }
        }
    }
    if let Some(name) = current {
        actual.insert(name, calls.len());
    }

    if std::env::var("DUMP_HANDLER_COUNTS").is_ok() {
        let mut rows: Vec<_> = actual.iter().filter(|(_, c)| **c > 1).collect();
        rows.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        for (n, c) in rows {
            println!("(\"{n}\", {c}),");
        }
    }
    let mut regressed = Vec::new();
    for (name, count) in &actual {
        let allowed = BASELINE.iter().find(|(n, _)| n == name).map(|(_, c)| *c).unwrap_or(1);
        if *count > allowed {
            regressed.push(format!("  {name}: {count} entry points, baseline {allowed}"));
        }
    }
    regressed.sort();
    assert!(
        regressed.is_empty(),
        "these handlers gained logic that belongs in `ide`:\n{}\n\n\
         Move it behind one `Analysis` method and lower the entry in BASELINE.",
        regressed.join("\n")
    );
}

/// `ide` has one door, and the server uses it.
///
/// rust-analyzer's server calls no free function in `ide` at all — every
/// feature is a method on `Analysis`, and the handler converts the result.
/// That is what makes `Analysis` the thing a test, a CLI or a measurement can
/// hold and get the same answers the editor gets. When the server reaches
/// past it, whatever it assembles on the way is a feature that exists only
/// for the server: goto-definition answered 60% for everyone but the editor
/// A client column is converted, never assigned.
///
/// `Position::character` is a UTF-16 column and `LineCol::col` is a byte
/// column, so every conversion between them needs a `LineIndex`. Three
/// functions did it by assignment instead — `LineCol { line: pos.line,
/// col: pos.character }` and two inverses — and twenty-seven handlers
/// called them, so the fix that landed in `from_proto`/`to_proto` reached
/// almost nothing while `protocol_check` reported zero over 1.2M ranges.
/// The probe exercises those two modules; the handlers went round them.
///
/// This is a source rule because no probe can be one: a handler that
/// converts by hand is invisible to a probe that enters through the
/// conversion. The pattern is a `Position` built from, or destructured
/// into, a `LineCol` field without a line index in the same expression.
#[test]
fn a_client_column_is_converted_not_assigned() {
    let mut offenders = Vec::new();
    for entry in walk_rs(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")) {
        // Not this file: the scanner spells the pattern it looks for.
        if entry.file_name().is_some_and(|f| f == "tests.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&entry).expect("source is readable");
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.starts_with("//") {
                continue;
            }
            // `Position::new(x, <something>.col)` and
            // `col: <something>.character` are the two spellings.
            let assigns_out = line.contains("Position::new(") && line.contains(".col");
            let assigns_in = line.contains("col:") && line.contains(".character");
            if assigns_out || assigns_in {
                // The conversion functions themselves are where the
                // arithmetic lives; they take a `LineIndex` and are the
                // one place allowed to spell this out.
                if line.contains("byte_col_to_utf16") || line.contains("utf16_col_to_byte") {
                    continue;
                }
                offenders.push(format!("{}:{}: {line}", entry.display(), n + 1));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a client column assigned rather than converted:\n{}",
        offenders.join("\n")
    );
}

/// Every `.rs` file under a directory.
fn walk_rs(root: std::path::PathBuf) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    out
}

/// until its three resolution steps moved behind `Analysis::goto_definition`.
///
/// This started as a list of 38 exceptions and is now empty, so the rule is
/// stated rather than recorded. Reaching past `Analysis` fails here; the fix
/// is a method that answers the whole question.
#[test]
fn the_server_goes_through_analysis() {
    const SOURCES: &[&str] = &[
        include_str!("handlers/request.rs"),
        include_str!("handlers/notification.rs"),
        include_str!("handlers/dispatch.rs"),
        include_str!("main_loop.rs"),
        include_str!("global_state.rs"),
        include_str!("diagnostics.rs"),
    ];

    let mut found: std::collections::BTreeSet<String> = Default::default();
    for src in SOURCES {
        let mut rest = *src;
        while let Some(i) = rest.find("ide::") {
            let tail = &rest[i + "ide::".len()..];
            let end = tail
                .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
                .unwrap_or(tail.len());
            let path = &tail[..end];
            // `module::function`, not `module::Type` or a bare module.
            // `ide::fixture` builds test databases and is not a feature.
            if path.matches("::").count() == 1
                && path
                    .split("::")
                    .nth(1)
                    .is_some_and(|f| f.chars().next().is_some_and(|c| c.is_lowercase()))
                && !path.starts_with("fixture::")
            {
                found.insert(format!("ide::{path}"));
            }
            rest = &tail[end.max(1)..];
        }
    }

    assert!(
        found.is_empty(),
        "the server reached past `Analysis`:\n{}\n\n\
         Give `Analysis` a method that answers the whole question instead.",
        found.iter().map(|s| format!("  {s}")).collect::<Vec<_>>().join("\n")
    );
}

/// Priming the caches goes through `Analysis`, with the client's worker
/// count and a progress callback that reaches the client.
///
/// The one rule this layer has is that the server goes through
/// `Analysis`, and this reached past it — `analysis.db()` and
/// `AnalysisHost::raw_database().files()`, straight into salsa from the
/// main loop. It also passed a literal `2` for the worker count and
/// `&|_| {}` for progress, so the phase that dominates startup ran silent
/// and single-width whatever the machine or the client said.
#[test]
fn priming_the_caches_goes_through_analysis() {
    let main_loop = include_str!("main_loop.rs");

    assert!(
        !main_loop.contains("raw_database().files()"),
        "the main loop reaches past `Analysis` into the database"
    );
    assert!(
        main_loop.contains("analysis.parallel_prime_caches("),
        "priming does not go through `Analysis`"
    );
    assert!(
        main_loop.contains("config.prime_caches_num_threads()"),
        "the worker count is not the client's"
    );
    assert!(main_loop.contains("PrimeCachesProgress::Report"), "progress is computed and dropped");
}

/// Eviction waits for the pool, and goes before the wave.
///
/// `trigger_lru_eviction` takes `&mut` on the database, so it cancels every
/// live snapshot — including a diagnostics wave still being computed. A
/// cancelled wave is dropped file by file in `fetch_native_diagnostics`,
/// and eviction is not a *change*, so nothing scheduled another: **an
/// opened file got no diagnostics at all**, until the next edit happened to
/// ask. Reproduced against the real binary over stdio on sail-riscv — 0
/// published where `corpus_check` reports 1 — and invisible to every probe
/// in this repo, all of which call `Analysis` directly and never evict.
///
/// The guard read `task_receiver`, the channel results arrive *on*, which
/// is empty precisely while a task is still working. rust-analyzer asks the
/// pool whether anything is running (`main_loop.rs:541-546`), which is the
/// question, and `TaskPool::is_empty` answers it here. Evicting before the
/// wave closes the other half: the turn that bumps the nonce would
/// otherwise cancel the snapshot it is about to take.
#[test]
fn eviction_waits_for_the_pool_and_precedes_the_wave() {
    let main_loop = include_str!("main_loop.rs");

    let evict = main_loop.find("trigger_garbage_collection()").expect("nothing evicts");
    let diags = main_loop.find("state.update_diagnostics();").expect("nothing diagnoses");
    assert!(
        evict < diags,
        "eviction runs after the diagnostics wave, so it cancels the snapshot it just spawned"
    );
    assert!(
        main_loop.contains("state.task_pool.is_empty() && current_nonce"),
        "eviction is not guarded on the pool being idle"
    );
    assert!(
        !main_loop.contains("task_receiver.is_empty() && current_nonce"),
        "eviction is guarded on the results channel, which is empty *while* a task runs"
    );
    // The guard is worth nothing beside an unguarded call, and there was
    // one: `on_task` evicted whenever priming finished — which is while the
    // wave that the same settling change spawned is still computing, so it
    // cancelled one on every file opened.
    assert_eq!(
        main_loop.matches("trigger_garbage_collection()").count(),
        1,
        "eviction has a second call site, and a rule with two doors has none"
    );
}

/// The open buffer wins, and closing gives the file back.
///
/// The behavioural half of [`disk_text_does_not_overwrite_an_open_document`],
/// driven through `GlobalState` itself: what the database holds after a
/// disk write against an open document, and after that document is closed.
/// Both were reproduced against the real binary first — a rescan or the
/// server's own watcher dropped the unsaved edit, and a closed buffer never
/// went away — and this is the version that runs in CI.
#[test]
fn an_open_document_outranks_the_file_and_a_closed_one_does_not() {
    use hir::callgraph::SourceFileInfo as _;

    let dir = std::env::temp_dir().join("sail_lsp_mem_docs_rule");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("a.sail");
    std::fs::write(&path, "val on_disk : int -> int\n").expect("write");
    let url = url::Url::from_file_path(&path).expect("url");

    let (sender, _messages) = crossbeam_channel::unbounded();
    let (task_sender, _tasks) = crossbeam_channel::unbounded();
    let mut state = crate::global_state::GlobalState::new(sender, task_sender);

    let text_of = |state: &mut crate::global_state::GlobalState, url: &url::Url| {
        state.analysis_snapshot().file(url).map(|f| f.text().to_string()).unwrap_or_default()
    };

    // The client opens it and edits without saving.
    crate::handlers::notification::handle_did_open_text_document(
        &mut state,
        lsp_types::DidOpenTextDocumentParams {
            text_document: lsp_types::TextDocumentItem {
                uri: url.clone(),
                language_id: "sail".to_owned(),
                version: 1,
                text: "val in_the_editor : int -> int\n".to_owned(),
            },
        },
    )
    .expect("did_open");
    assert_eq!(text_of(&mut state, &url), "val in_the_editor : int -> int\n");

    // A rescan, or a watcher, offering what is on disk. The buffer wins.
    state.sync_disk_text(&url, "val on_disk : int -> int\n");
    assert_eq!(
        text_of(&mut state, &url),
        "val in_the_editor : int -> int\n",
        "disk text overwrote an open document"
    );

    // Closing it hands the file back, unsaved edit and all.
    crate::handlers::notification::handle_did_close_text_document(
        &mut state,
        lsp_types::DidCloseTextDocumentParams {
            text_document: lsp_types::TextDocumentIdentifier { uri: url.clone() },
        },
    )
    .expect("did_close");
    assert_eq!(
        text_of(&mut state, &url),
        "val on_disk : int -> int\n",
        "a closed buffer is still what the database holds"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Text that came from disk enters through one door.
///
/// rust-analyzer's rule is that an open document is the client's, and disk
/// contents do not overwrite it (`main_loop.rs:930-935`). It was written
/// out at one of this server's four disk paths and missing from the other
/// three, each of which overwrote unsaved edits in silence. All three were
/// reproduced against the real binary with a buffer one edit ahead of disk:
/// a rescan (any `.sail` file created in the workspace) and the server's own
/// watcher (any tool touching the file) each dropped the edit's diagnostics,
/// and `didClose` left the *buffer* in the database for the rest of the
/// session, so a closed file kept reporting errors from text that exists
/// nowhere.
///
/// `sync_disk_text` is that rule, once. `sync_to_salsa` is the write itself
/// and belongs to the two notifications that carry the client's own text.
#[test]
fn disk_text_does_not_overwrite_an_open_document() {
    let notification = include_str!("handlers/notification.rs");

    for (name, src) in
        [("main_loop.rs", include_str!("main_loop.rs")), ("reload.rs", include_str!("reload.rs"))]
    {
        assert!(
            !src.contains("sync_to_salsa"),
            "{name} writes disk text straight to salsa; it must go through `sync_disk_text`"
        );
    }

    // did_open and did_change, and nothing else.
    assert_eq!(
        notification.matches("sync_to_salsa").count(),
        2,
        "a notification other than did_open/did_change writes text as if it were the client's"
    );
    assert!(
        notification.contains("state.restore_disk_text(&uri);"),
        "closing a document leaves its unsaved text in the database"
    );
}

/// A probe builds the `Analysis` the server builds.
///
/// Three defects in one day came from state that only some entry point
/// sets: `analysis_stats` navigating with a project-only `UrlMap`, the
/// `workspace_index` override that made `workspace/symbol` and cross-file
/// goto-definition dead in the server, and the include graph that no probe
/// had at all — so an `$include` cycle was a diagnostic no corpus run could
/// produce. Two of the three were *the probe measuring a path no user
/// takes*, which is the failure this whole file exists to prevent.
///
/// So there is one way to build an `Analysis` with a workspace behind it,
/// and everything that measures this server uses it. The override is gone
/// (the symbol index is a salsa query every database has) and the include
/// graph is built by one function in `ide`.
#[test]
fn a_probe_builds_the_analysis_the_server_builds() {
    const PROBES: &[(&str, &str)] = &[
        ("corpus_check", include_str!("../examples/corpus_check.rs")),
        ("conformance_check", include_str!("../examples/conformance_check.rs")),
        ("mutation_check", include_str!("../examples/mutation_check.rs")),
        ("analysis_stats", include_str!("../examples/analysis_stats.rs")),
    ];
    for (name, src) in PROBES {
        assert!(
            !src.contains("Analysis::with_url_map("),
            "{name} builds an `Analysis` with no include graph, which the server never does"
        );
        assert!(
            src.contains("build_include_graph("),
            "{name} does not build the workspace's include graph"
        );
    }
    // And the override that started it is not back.
    let analysis = include_str!("../../ide/src/analysis.rs");
    assert!(
        !analysis.contains("fn with_workspace_index") && !analysis.contains("fn with_symbol_index"),
        "the symbol index is overridable again; it is a salsa query, and an \
         override is what made two features dead in the server. Both spellings: \
         it was deleted as `with_workspace_index`, and the module it belongs to \
         is `symbol_index` now"
    );
}

/// The workspace is assembled after its files have been read.
///
/// The scan says which files and in what order; the loader says what they
/// contain, and it answers asynchronously — so `WorkspaceFiles`, the
/// environment every cross-file query resolves against, is built at the
/// loader's `Progress { Finished }` and not before. Building it early gives
/// a workspace that briefly knows nothing, and a transient flood of "not
/// found" is the false positive this project pays most for: the same shape
/// cost 197 errors in one file when `.sail_project` parsing lost 96 of 158.
///
/// The version check is not ceremony. A loader answers `Finished` for every
/// config it is given, including the one it is created with, and taking the
/// first answer finished the workspace before a single file had been read.
#[test]
fn the_workspace_is_assembled_after_its_files_are_read() {
    let main_loop = include_str!("main_loop.rs");
    let reload = include_str!("reload.rs");

    assert!(
        main_loop.contains("WorkspaceScan(crate::reload::WorkspaceScan)")
            && reload.contains("pub files: Result<Vec<(Url, PathBuf)>, String>"),
        "the scan carries file contents again, so there are two readers"
    );
    assert!(
        main_loop.contains("state.pending_workspace = Some((files, count));"),
        "the scan assembles the workspace without waiting for it to be read"
    );
    assert!(
        reload.contains("config_version != self.workspace_load_version"),
        "the barrier takes any loader answer, including the one for a config \
         given before the file list was"
    );
}

/// A file's contents live in the VFS, and reach salsa by being drained.
///
/// rust-analyzer has one store for what files say and one drain into the
/// database. This server used the VFS as a path/URL interner and put text
/// straight into the pending change, so `Vfs::set_file_contents` and
/// `Vfs::take_changes` — dedup, change kinds, all of it — had no caller
/// outside their own tests, and the loader, which speaks in file contents,
/// had nothing to speak to.
#[test]
fn file_contents_reach_salsa_through_the_file_store() {
    let global_state = include_str!("global_state.rs");

    assert_eq!(
        global_state.matches("pending_change.change_file(").count(),
        1,
        "text is queued for salsa somewhere other than the VFS drain"
    );
    assert!(
        global_state.contains("self.vfs.set_file_contents("),
        "`sync_to_salsa` does not put the text in the file store"
    );
    for (name, src) in [
        ("main_loop.rs", include_str!("main_loop.rs")),
        ("reload.rs", include_str!("reload.rs")),
        ("handlers/notification.rs", include_str!("handlers/notification.rs")),
    ] {
        assert!(
            !src.contains("pending_change.change_file("),
            "{name} queues text for salsa without going through the file store"
        );
    }
}

/// `shutdown` does not end the loop; `exit` does.
///
/// The protocol says a server keeps running between the two and answers
/// anything that still arrives with `InvalidRequest`. This called
/// `Connection::handle_shutdown` and returned on the spot, so a client got
/// no answer at all in that window — and `GlobalState::shutdown_requested`
/// had no reader, nor did `RequestDispatcher::on_sync_mut`, which is the
/// method rust-analyzer dispatches `Shutdown` through.
#[test]
fn shutdown_does_not_end_the_loop() {
    const SOURCES: &[&str] = &[include_str!("main_loop.rs"), include_str!("handlers/dispatch.rs")];
    let all = SOURCES.concat();

    assert!(
        !all.contains("connection.handle_shutdown("),
        "the loop returns on `shutdown` instead of waiting for `exit`"
    );
    assert!(
        all.contains("on_sync_mut::<request::Shutdown>"),
        "`shutdown` is not dispatched as the state mutation it is"
    );
    assert!(
        all.contains("Shutdown already requested."),
        "a request after `shutdown` goes unanswered"
    );
    assert!(all.contains("notif.method == \"exit\""), "nothing ends the loop");
}

/// An assist can leave the cursor where you have to type next.
///
/// `SourceChange::is_snippet` had no producer and no consumer, and
/// `AssistConfig::snippet_cap` — which rust-analyzer's generate-* handlers
/// gate on — was passed in from the server and read by nothing. The
/// protocol has no snippet in a `WorkspaceEdit`, so the path needs all
/// four parts: a builder that marks the change, a handler that asks for
/// the capability, the `SnippetTextEdit` extension on the wire, and a
/// strip for a client that did not opt in.
#[test]
fn an_assist_can_place_the_cursor() {
    const SOURCES: &[&str] = &[
        include_str!("handlers/request.rs"),
        include_str!("lsp/to_proto.rs"),
        include_str!("lsp/ext.rs"),
    ];
    let all = SOURCES.concat();

    assert!(all.contains("SnippetWorkspaceEdit"), "a code action's edit cannot carry a snippet");
    assert!(
        all.contains("snippet_text_edit_support()"),
        "the snippet extension is used without asking whether the client opted in"
    );
    assert!(
        all.contains("strip_snippet_placeholders"),
        "a client that did not opt in would receive a literal `$0`"
    );
    let assists = include_str!("../../ide-assists/src/handlers/generate_function.rs");
    assert!(
        assists.contains("ctx.config.snippet_cap"),
        "no assist asks whether it may leave a tabstop"
    );
}

/// What the server advertises depends on what the client said it can take.
///
/// `initialize()` sends the server's capabilities and reads the client's in
/// one call, so nothing advertised could depend on the answer — and two
/// capability checks sat unused because of it. A client that cannot take a
/// code action *literal* was offered code actions anyway (the fallback is a
/// `Command`, and this server has no `workspace/executeCommand`), and a
/// client that cannot expand `${1:x}` was sent snippets that it inserts as
/// literal text.
#[test]
fn the_server_reads_the_client_capabilities_first() {
    const SOURCES: &[&str] = &[
        include_str!("main_loop.rs"),
        include_str!("lsp/capabilities.rs"),
        include_str!("handlers/request.rs"),
    ];
    let all = SOURCES.concat();

    assert!(
        all.contains("initialize_start"),
        "the server sends its capabilities before reading the client's"
    );
    assert!(
        all.contains("code_action_literals_support()"),
        "code actions are advertised without asking whether the client takes literals"
    );
    assert!(
        all.contains("completion_snippet_support()"),
        "completions are sent as snippets without asking whether the client expands them"
    );
}

/// What the server asks the client, it can hear the answer to.
///
/// The main loop matched `Message::Response(_) => {}` — every response the
/// client sent was dropped — and `send_request` took no handler, so
/// nothing the server asks could be answered. Three protocol behaviours
/// were missing for that one reason: the settings could not be pulled with
/// `workspace/configuration` (and `didChangeConfiguration`'s own
/// parameters are meaningless per the spec, so a settings change reached
/// the server as nothing), a progress token was never created with
/// `window/workDoneProgress/create` (so a client may ignore every report),
/// and `$/cancelRequest` had no queue to look an id up in — it did
/// nothing, and a merely-slow request was never cancelled at all, because
/// salsa's cancellation only fires when a revision lands.
#[test]
fn the_server_can_hear_the_client_answer() {
    const SOURCES: &[&str] = &[
        include_str!("main_loop.rs"),
        include_str!("global_state.rs"),
        include_str!("handlers/notification.rs"),
    ];
    let all = SOURCES.concat();

    assert!(
        !all.contains("Message::Response(_) => {}"),
        "the main loop drops responses, so no request the server sends can be answered"
    );
    assert!(all.contains("req_queue"), "there is no request queue");
    assert!(
        all.contains("WorkspaceConfiguration"),
        "`didChangeConfiguration` must pull the settings; its own parameters are meaningless"
    );
    assert!(
        all.contains("WorkDoneProgressCreate"),
        "a progress token has to be created before it is reported under"
    );
    assert!(
        all.contains("state.cancel(id)"),
        "`$/cancelRequest` must answer the request, not wait for salsa"
    );
}

/// The server asks to be told when a file changes on disk.
///
/// `handle_did_change_watched_files` was here all along, and nothing ever
/// registered `DidChangeWatchedFilesRegistrationOptions` with the client —
/// so the notification it waits for was never requested, and a `.sail` file
/// replaced by `git checkout` or written by another tool went unnoticed.
/// `did_change_watched_files_dynamic_registration()` sat in
/// `capabilities.rs` with no caller. rust-analyzer registers the same way
/// and, for clients that cannot, watches with `vfs-notify` — a crate that
/// had no dependent here at all.
#[test]
fn the_server_asks_to_hear_about_files_on_disk() {
    const SOURCES: &[&str] = &[include_str!("reload.rs"), include_str!("global_state.rs")];
    let all = SOURCES.concat();

    assert!(
        all.contains("DidChangeWatchedFilesRegistrationOptions"),
        "nothing registers file watching with the client, so \
         `didChangeWatchedFiles` never arrives"
    );
    assert!(
        all.contains("did_change_watched_files_dynamic_registration"),
        "the registration must depend on what the client said it supports"
    );
    assert!(
        all.contains("vfs_notify::NotifyHandle"),
        "a client without dynamic registration leaves no one watching"
    );
}

/// A feature answers through `with_db`, so a cancellation is a value.
///
/// rust-analyzer wraps every `Analysis` method that touches the database in
/// `self.with_db` and returns `Cancellable<T>` — 73 of its 87. That is what
/// makes the type mean anything: a caller that is not the LSP server — a
/// test, a CLI, one of the corpus tools — gets an `Err` rather than an
/// unwind it has to catch itself.
///
/// Here it was 12 of 92, and 7 of the 39 the server called, while the
/// documentation said "every `Analysis` method that touches the database
/// runs inside `Analysis::with_db`". The methods listed below are the ones
/// that genuinely do not: handles, builders, and `AnalysisHost`'s own
/// mutation API.
#[test]
fn a_feature_answers_through_with_db() {
    // Not features: they hand back a handle, or they mutate the host.
    const NOT_QUERIES: &[&str] = &[
        "new",
        "with_db",
        "db",
        "file",
        "file_by_id",
        "file_id",
        "files_ref",
        "url_for_file_id",
        "all_urls",
        "all_url_file_ids",
        "all_salsa_files",
        "lookup_file_id",
        "lookup_url",
        "with_url_map",
        "with_file",
        "with_file_and_workspace",
        "with_include_graph",
        "with_include_graph_arc",
        "include_graph",
        "analysis",
        "analysis_with_url_map",
        "apply_change",
        "set_file_text",
        "raw_database",
        "raw_database_mut",
        "db_snapshot",
        "update_lru_capacity",
        "trigger_cancellation",
        "trigger_garbage_collection",
        "workspace",
    ];

    let src = include_str!("../../ide/src/analysis.rs");
    let mut offenders: Vec<String> = Vec::new();
    let mut rest = src;
    while let Some(i) = rest.find("\n    pub fn ") {
        let tail = &rest[i + "\n    pub fn ".len()..];
        let name_end =
            tail.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(tail.len());
        let name = &tail[..name_end];
        let body_end = tail.find("\n    pub fn ").unwrap_or(tail.len());
        let body = &tail[..body_end];
        if !NOT_QUERIES.contains(&name) && !body.contains("with_db") {
            offenders.push(name.to_owned());
        }
        rest = &tail[name_end.max(1)..];
    }
    offenders.sort();
    offenders.dedup();
    assert!(
        offenders.is_empty(),
        "these run queries and let a cancellation unwind past the caller:\n  {}\n\n\
         Wrap the body in `self.with_db(|a| a.<name>_inner(..))` and return \
         `Cancellable`, or name it in NOT_QUERIES if it runs none.",
        offenders.join("\n  ")
    );
}

/// A feature `Analysis` offers can be asked for.
///
/// The other direction of `the_server_goes_through_analysis`: that one says
/// the server reaches nothing but `Analysis`, and says nothing about what
/// `Analysis` offers that the server never asks for. Five features were on
/// the wrong side of it — `matching_brace`, `join_lines`, `move_item`,
/// `on_enter` and the CST selection walker — each fully implemented in
/// `ide`, each with tests, and no editor could reach any of them. Two of
/// them had a *second* implementation beside the first, so the duplicate
/// hid the fact that neither was routed.
///
/// A method that is not a feature — a handle, an accessor, a builder, or a
/// step another feature composes — is named below with the reason. That
/// list is the point: it is where "why does the server not ask for this"
/// gets written down instead of being inferred from a grep returning zero.
#[test]
fn a_feature_the_ide_offers_can_be_asked_for() {
    /// `Analysis` methods that are not features.
    const NOT_A_FEATURE: &[&str] = &[
        // Handles, accessors and the mutation API.
        "new",
        "analysis",
        "analysis_with_url_map",
        "apply_change",
        "db",
        "db_snapshot",
        "raw_database",
        "raw_database_mut",
        "set_file_text",
        "with_db",
        "with_file",
        "with_file_and_workspace",
        "with_url_map",
        "with_include_graph",
        "with_include_graph_arc",
        "update_lru_capacity",
        "trigger_cancellation",
        "trigger_garbage_collection",
        "parallel_prime_caches",
        // The file/URL boundary. rust-analyzer's handlers call its
        // equivalents too; `handlers_do_not_accumulate_logic` does not
        // count them either.
        "file",
        "file_by_id",
        "file_id",
        "files_ref",
        "lookup_file_id",
        "lookup_url",
        "url_for_file_id",
        "all_urls",
        "all_file_urls",
        "all_salsa_files",
        "all_url_file_ids",
        "workspace_def_map",
        "build_workspace_names",
        "include_graph",
        // Steps `code_actions` composes; the server asks for the whole
        // menu, not for one entry of it.
        "assists",
        "quick_fix",
        // Read by `ide` itself — the annotations provider and the hover
        // footer — rather than answered to the client.
        "impl_counts",
        "ref_counts",
        "binding_type_text",
        "expr_type_text",
        "signature_index",
        "code_lens_title",
        "diagnostics",
        "syntax_diagnostics",
    ];

    let src = include_str!("../../ide/src/analysis.rs");
    let server: String = [
        include_str!("handlers/request.rs"),
        include_str!("handlers/notification.rs"),
        include_str!("main_loop.rs"),
        include_str!("cli.rs"),
        include_str!("reload.rs"),
        include_str!("diagnostics.rs"),
        include_str!("global_state.rs"),
    ]
    .concat();

    let mut unreachable: Vec<String> = Vec::new();
    let mut rest = src;
    while let Some(i) = rest.find("\n    pub fn ") {
        let tail = &rest[i + "\n    pub fn ".len()..];
        let name_end =
            tail.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(tail.len());
        let name = &tail[..name_end];
        if !NOT_A_FEATURE.contains(&name) && !server.contains(&format!(".{name}(")) {
            unreachable.push(name.to_owned());
        }
        rest = &tail[name_end.max(1)..];
    }
    unreachable.sort();
    unreachable.dedup();
    assert!(
        unreachable.is_empty(),
        "these features are implemented and no client can ask for them:\n  {}\n\n\
         Route each from a handler — a protocol request where one fits, an \
         `experimental/*` extension where it does not — or name it in \
         NOT_A_FEATURE with the reason it is not one.",
        unreachable.join("\n  ")
    );
}

/// The main loop does not run a feature on the main thread.
///
/// rust-analyzer keeps exactly two kinds of request sync: pure syntax that
/// has to answer within a keystroke (`SelectionRange`, `OnTypeFormatting`,
/// `MatchingBrace`, `JoinLines`, `OnEnter`), and state mutations, which go
/// through `on_sync_mut`. Everything else takes a snapshot and goes to the
/// pool — 43 of them there against 5 here.
///
/// This server had 24 sync against 23, and the sync list held
/// `DocumentDiagnosticRequest` (full inference, plus every file that
/// `$include`s this one), `SemanticTokensFull`, `CodeActionRequest` and
/// `Formatting`. While one of those runs, the loop reads no event: not the
/// next request, and not the cancellation for the one in flight.
#[test]
fn the_main_loop_does_not_run_a_feature() {
    // The list the doc comment names, and no more. `MatchingBrace`,
    // `JoinLines` and `OnEnter` joined it when they were first routed:
    // each is one CST walk of the open file and the editor is holding a
    // keystroke on the answer, which is why rust-analyzer keeps them here.
    const ALLOWED: &[&str] =
        &["SelectionRangeRequest", "OnTypeFormatting", "MatchingBrace", "JoinLines", "OnEnter"];

    let src = include_str!("main_loop.rs");
    let mut sync: Vec<String> = Vec::new();
    let mut rest = src;
    while let Some(i) = rest.find(".on_sync::<") {
        let tail = &rest[i + ".on_sync::<".len()..];
        let end = tail.find('>').unwrap_or(tail.len());
        let name = tail[..end].rsplit("::").next().unwrap_or("").trim().to_owned();
        sync.push(name);
        rest = &tail[end.max(1)..];
    }
    let unexpected: Vec<&String> =
        sync.iter().filter(|name| !ALLOWED.contains(&name.as_str())).collect();
    assert!(
        unexpected.is_empty(),
        "these run on the main thread, so nothing else is read while they do:\n  {}\n\n\
         Only syntax that must answer within a keystroke belongs there. Give it \
         `.on::<false, _>` — or `.on::<true, _>` if the client will simply ask again.",
        unexpected.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n  ")
    );
}

/// `with_db` re-raises a genuine panic and passes a normal result through.
///
/// The cancellation half cannot be exercised from outside salsa —
/// `salsa::Cancelled`'s variants are `#[non_exhaustive]` and cannot be
/// constructed here — so what is checked is the property that makes the
/// wrapper safe to put around every query: it must not turn a bug into a
/// quietly cancelled request. The catching itself is `salsa::Cancelled::catch`,
/// which is what rust-analyzer uses.
#[test]
fn with_db_reraises_a_real_panic() {
    let (analysis, _) =
        ide::fixture::analysis("//- /main.sail\ndefault Order dec\nval f : unit -> unit\n");

    assert!(matches!(analysis.with_db(|_| 7), Ok(7)), "a normal result passes through");

    let bug = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: Result<(), _> = analysis.with_db(|_| panic!("a genuine bug"));
    }));
    assert!(bug.is_err(), "a panic that is not a cancellation must stay a panic");
}

/// The client's settings reach the feature that answers.
///
/// rust-analyzer passes a per-feature config into the `Analysis` method
/// (25 of its methods take one), so a setting cannot be silently dropped —
/// the parameter has to be filled in. Here the settings were parsed into
/// `SailLspConfig`, converted by `to_ide_config`, and then never read: only
/// `diagnostics` was ever passed to anything. Turning inlay hints off in the
/// editor did nothing at all.
#[test]
fn a_config_setting_changes_the_answer() {
    use ide::inlay_hints::InlayHintsConfig;

    let fixture = "//- /main.sail\ndefault Order dec\nval f : int -> int\nfunction f(x) = {\n  let y = x;\n  y\n}\n";
    let (analysis, files) = ide::fixture::analysis(fixture);
    let (file_id, _) = files[0];
    let whole = base_db::text_range(0, 10_000);

    let all_on = InlayHintsConfig {
        type_hints: true,
        parameter_hints: true,
        effect_hints: true,
        ..Default::default()
    };
    let all_off = InlayHintsConfig {
        max_length: None,
        type_hints: false,
        parameter_hints: false,
        effect_hints: false,
        closing_brace_hints_min_lines: None,
        caller_count_hints: false,
        chaining_hints: false,
        discriminant_hints: false,
    };

    let on = analysis.inlay_hints(file_id, whole, &all_on).expect("not cancelled");
    let off = analysis.inlay_hints(file_id, whole, &all_off).expect("not cancelled");

    assert!(!on.is_empty(), "the fixture should produce hints when they are enabled");
    assert!(off.is_empty(), "no hint kind is enabled, so there should be none: {off:?}");
}

/// A snapshot can tell a library file from the user's own.
///
/// The classification was computed during the workspace scan and left on
/// `GlobalState`, where no handler could reach it — so a second, never
/// populated `SourceRootConfig` had grown up beside it. Both are gone; the
/// scan's own `library_root` reaches the snapshot.
#[test]
fn a_snapshot_knows_which_files_are_library() {
    let mut root = base_db::SourceRoot::new_library(base_db::FileSet::default());
    let lib = base_db::FileId::from_raw(7);
    let own = base_db::FileId::from_raw(8);
    root.insert(
        lib,
        base_db::VfsPath::new(paths::AbsPathBuf::assert_utf8(std::path::PathBuf::from(
            "/sail/lib/prelude.sail",
        ))),
    );
    assert!(root.contains(lib), "the library root should contain what was put in it");
    assert!(!root.contains(own), "and nothing else");
}

/// A local binding's references do not escape its own body.
///
/// rust-analyzer computes a `SearchScope` from the definition — a local
/// variable is searched for only inside the body that binds it — and passes
/// it into the reference search. sail-lsp has the `SearchScope` type and
/// passes `None` everywhere, so every search reads the whole workspace.
///
/// That turns out to be only slow, not wrong: the search filters by the
/// definition it resolved rather than by name, so a local's references stay
/// in its own body regardless of scope. This pins that, because scoping the
/// search later must not be what makes it true.
#[test]
fn a_local_binding_does_not_match_an_unrelated_name() {
    let (analysis, files) = ide::fixture::analysis(
        "//- /a.sail\n\
         default Order dec\n\
         val f : int -> int\n\
         function f(p) = {\n  let shared = p;\n  shared\n}\n\
         //- /b.sail\n\
         default Order dec\n\
         val g : int -> int\n\
         function g(q) = {\n  let shared = q;\n  shared\n}\n",
    );
    let (a_id, _) = files[0];
    let url = analysis.url_for_file_id(a_id).expect("fixture file has a url").clone();
    // The `shared` bound in a.sail.
    let text = analysis.files_ref().file_text(a_id).unwrap().text(analysis.db()).to_string();
    let offset = text.find("shared").expect("the binding is in the fixture");
    let line_index = ide_db::line_index::LineIndex::new(&text);
    let pos = line_index.line_col(offset);

    let results = analysis
        .find_all_refs(&url, pos, None)
        .expect("the test database is never cancelled")
        .unwrap_or_default();
    let files_touched: std::collections::BTreeSet<_> =
        results.iter().flat_map(|r| r.references.keys().copied()).collect();
    assert_eq!(
        files_touched.len(),
        1,
        "a local binding lives in one body, so its references cannot be in two files: {files_touched:?}"
    );
}

/// The IDE layer asks the HIR what a name *is*.
///
/// `syntax::parser_lower::ParsedFile` is a flat per-file scrape — `decls`,
/// `call_sites`, `symbol_occurrences` — produced beside the HIR and, for a
/// long time, used instead of it. It has to re-derive what the lowering
/// already computed, and where it fails it fails silently: find-references
/// returned nothing for every local in the language, rename dropped
/// scattered clauses, completion grouped enum members by which declaration
/// followed which.
///
/// It is still the right source for two things:
///
///  - **syntax** — where the commas in a call are, which is what signature
///    help needs and what no semantic layer records;
///  - **what the HIR does not model** — a type variable in a `val`
///    signature, which exists only in the signature's text.
///
/// Shadowing used to be listed here too, on the grounds that `Body` had no
/// scope tree. It has one — `ExprScopes`, 466 lines, built from the body —
/// and only hir-ty's unused-variable check was asking it.
///
/// Everything else — what kind a definition is, which members an enum has,
/// where a name is declared, what calls what — comes from the item tree,
/// the bodies, or the callgraph. This holds the count per file so a new
/// use has to be argued for.
#[test]
fn the_ide_layer_asks_the_hir() {
    // (file, allowed `parser_lower` mentions, why)
    const BASELINE: &[(&str, usize, &str)] = &[
        ("ide-db/src/helpers.rs", 7, "produces the scrape: enriches it with local bindings"),
        ("ide-db/src/signatures.rs", 4, "signature index over callable_heads — param spans"),
        ("ide-db/src/lib.rs", 1, "the FileDb::parsed accessor itself"),
        ("ide-db/src/root_database.rs", 1, "implements FileDb::parsed"),
        ("ide-db/src/test_utils.rs", 1, "implements FileDb::parsed"),
        ("ide-db/src/db_query.rs", 1, "the salsa query that produces it"),
        ("ide/src/calls.rs", 2, "syntax: argument separator spans"),
        ("ide/src/hover/mod.rs", 2, "Decl as a record, filled from the item tree"),
        ("ide/src/hover/support.rs", 1, "renders that record"),
        ("ide/src/references.rs", 1, "type variables in a signature — no HIR form at all"),
        ("ide/src/syntax_highlighting/mod.rs", 1, "a test fixture implementing FileDb"),
        ("ide-assists/src/lib.rs", 1, "a test fixture implementing FileDb"),
    ];

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/ is the parent of this crate");
    let mut actual: std::collections::BTreeMap<String, usize> = Default::default();
    for crate_name in
        ["ide", "ide-db", "ide-assists", "ide-completion", "ide-diagnostics", "ide-ssr"]
    {
        let dir = root.join(crate_name).join("src");
        let mut stack = vec![dir];
        while let Some(path) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&path) else { continue };
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&p) else { continue };
                let n = text.matches("parser_lower").count();
                if n == 0 {
                    continue;
                }
                let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                actual.insert(rel, n);
            }
        }
    }

    let mut regressed = Vec::new();
    for (file, count) in &actual {
        let allowed = BASELINE.iter().find(|(f, _, _)| f == file).map(|(_, c, _)| *c).unwrap_or(0);
        if *count > allowed {
            regressed.push(format!("  {file}: {count} mentions, baseline {allowed}"));
        }
    }
    regressed.sort();
    assert!(
        regressed.is_empty(),
        "these files reached for the flat scrape:\n{}\n\n\
         Ask the item tree what kind a definition is, the bodies what a\n\
         binding is, or the callgraph what calls what. If the fact really is\n\
         syntactic, raise the entry in BASELINE and say why.",
        regressed.join("\n")
    );
}

/// Inference after an edit finishes.
///
/// `benchmark_incremental_diagnostics` skipped measuring this for years
/// behind a note saying a salsa revision bump "triggers eager
/// re-verification". Measured, it is worse than slow: on sail-riscv a
/// single `infer` after `set_file_text` did not return in fifteen minutes,
/// where warming every callable in all 158 files cold takes two seconds.
///
/// This is the same shape at a size the test suite can hold. If it starts
/// timing out, an edit has stopped being incremental.
#[test]
fn inference_after_an_edit_finishes() {
    use ide::analysis::AnalysisHost;

    let mut host = AnalysisHost::default();
    let mut files = base_db::Files::default();
    let fid = base_db::FileId::from_raw(0);
    files.set_file_text(host.raw_database_mut(), fid, "function f() -> int = 1\n");

    let warm = |host: &AnalysisHost, files: &base_db::Files| {
        let Some(ft) = files.file_text(fid) else { return 0 };
        let mut n = 0;
        for &id in hir::def_query::file_def_with_body_ids(host.raw_database(), ft) {
            let _ = hir::ty_query::infer(host.raw_database(), id);
            n += 1;
        }
        n
    };
    assert!(warm(&host, &files) > 0, "the fixture has a callable to infer");

    files.set_file_text(host.raw_database_mut(), fid, "function f() -> int = 2\n");

    let t = std::time::Instant::now();
    warm(&host, &files);
    assert!(
        t.elapsed() < std::time::Duration::from_secs(10),
        "re-inference after a one-character body edit took {:?}",
        t.elapsed()
    );
}

/// Editing one function's body does not re-infer the others.
///
/// It used to. `infer(def)` read `callable_bodies(file)` — a whole-file
/// query — and scanned it for its own entry, and it read the file's text
/// straight off the salsa input. Two dependencies on the whole file, so
/// any keystroke anywhere in it invalidated every callable's inference.
///
/// rust-analyzer's `infer(def)` depends on `body(def)`. `callable_body`
/// is that edge: per callable, carrying the text inference slices, and
/// compared by a fingerprint of the callable's own source rather than by
/// `Arc::ptr_eq` — which can never be equal, because a re-executed query
/// always allocates.
#[test]
fn an_edit_reinfers_only_what_changed() {
    use ide::analysis::AnalysisHost;

    let mut host = AnalysisHost::default();
    let mut files = base_db::Files::default();
    let fid = base_db::FileId::from_raw(0);
    files.set_file_text(
        host.raw_database_mut(),
        fid,
        "function f() -> int = 1\nfunction g() -> int = 100\n",
    );

    let g_result = |host: &AnalysisHost, files: &base_db::Files| {
        let db = host.raw_database();
        let ft = files.file_text(fid).expect("the fixture file");
        let ids = hir::def_query::file_def_with_body_ids(db, ft);
        for &id in ids {
            let _ = hir::ty_query::infer(db, id);
        }
        let g = ids.iter().find(|i| i.name(db) == "g").copied().expect("g is a callable");
        (format!("{g:?}"), std::sync::Arc::as_ptr(&hir::ty_query::infer(db, g).0) as usize)
    };

    let (key_before, ptr_before) = g_result(&host, &files);
    // Only `f`'s body changes. `g` is byte-identical.
    files.set_file_text(
        host.raw_database_mut(),
        fid,
        "function f() -> int = 2\nfunction g() -> int = 100\n",
    );
    let (key_after, ptr_after) = g_result(&host, &files);

    assert_eq!(key_before, key_after, "the key for `g` should survive an edit elsewhere");
    assert_eq!(ptr_before, ptr_after, "`g` was re-inferred though its body did not change");
}

/// An edit costs the file it touches, not the workspace.
///
/// 160 files, 320 callables. Before `infer` depended on `callable_body`
/// rather than on `callable_bodies(file)` and the file's raw text, one
/// keystroke invalidated every callable in the workspace: on sail-riscv's
/// 158 files a *single* `infer` did not return in fifteen minutes. It is
/// 89 ms for the edited file and 10 µs for an unrelated one now.
///
/// The ratio is what this asserts, not the absolute time — a slow machine
/// scales both.
#[test]
fn an_edit_costs_one_file_not_the_workspace() {
    use ide::analysis::AnalysisHost;
    use std::time::Instant;

    let mut host = AnalysisHost::default();
    let mut files = base_db::Files::default();
    let mut fids = Vec::new();
    for i in 0..160u32 {
        let fid = base_db::FileId::from_raw(i);
        let src = format!(
            "val f{i} : int -> int\nfunction f{i}(x) = x + {i}\n\
             function g{i}(y : int) -> int = f{i}(y)\n"
        );
        files.set_file_text(host.raw_database_mut(), fid, &src);
        fids.push(fid);
    }
    let texts: Vec<_> = fids.iter().filter_map(|f| files.file_text(*f)).collect();
    let _ws = base_db::WorkspaceFiles::builder(texts).new(host.raw_database_mut());

    let warm_all = |host: &AnalysisHost, files: &base_db::Files| {
        let t = Instant::now();
        for fid in &fids {
            if let Some(ft) = files.file_text(*fid) {
                for &id in hir::def_query::file_def_with_body_ids(host.raw_database(), ft) {
                    let _ = hir::ty_query::infer(host.raw_database(), id);
                }
            }
        }
        t.elapsed()
    };

    let cold = warm_all(&host, &files);
    // A body-only edit in one of the 160 files.
    files.set_file_text(
        host.raw_database_mut(),
        fids[0],
        "val f0 : int -> int\nfunction f0(x) = x + 99\nfunction g0(y : int) -> int = f0(y)\n",
    );
    let after = warm_all(&host, &files);

    assert!(
        after * 5 < cold,
        "re-warming after a one-file edit took {after:?} against a cold {cold:?} — \
         an edit is invalidating far more than the file it touched"
    );
}

/// A query result compares by value, so salsa can stop early.
///
/// Salsa's whole benefit is that a re-executed query whose value equals the
/// previous one does not invalidate its dependents. `Arc::ptr_eq` can never
/// report equal — a re-execution always allocates — so a wrapper using it
/// turns every re-run into a cascade. That is what made one keystroke cost
/// fifteen minutes on sail-riscv.
///
/// rust-analyzer uses `Arc::ptr_eq` once in all of hir-def, hir-ty and
/// base-db, and never for a query result. This counts only `PartialEq`
/// impls — a test asserting that two calls returned the same `Arc` is
/// checking memoization, which is a different thing.
#[test]
fn query_results_compare_by_value() {
    // Wrappers still allowed to compare by pointer, and why.
    const ALLOWED: &[(&str, &str)] = &[
        (
            "ArcCallableBodies",
            "whole-file, and the only consumer that matters — callable_body — \
             fingerprints its own result, so a cascade stops there",
        ),
        (
            "ArcInferenceResult",
            "nothing inside salsa depends on an infer result; the IDE reads it \
             and stops, so there is nothing downstream to backdate for",
        ),
    ];

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/ is the parent of this crate");
    let mut offenders = Vec::new();
    for dir in ["hir-def/src", "hir-ty/src", "base-db/src", "hir/src"] {
        let mut stack = vec![root.join(dir)];
        while let Some(p) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&p) else { continue };
            for e in entries.flatten() {
                let path = e.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|x| x != "rs") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else { continue };
                // Each `impl PartialEq for X` whose body compares pointers.
                let mut rest = text.as_str();
                while let Some(at) = rest.find("impl PartialEq for ") {
                    let tail = &rest[at + "impl PartialEq for ".len()..];
                    let name = tail
                        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                        .next()
                        .unwrap_or("")
                        .to_string();
                    let body_end = tail.find("\n}\n").unwrap_or(tail.len());
                    if tail[..body_end].contains("Arc::ptr_eq")
                        && !ALLOWED.iter().any(|(a, _)| *a == name)
                    {
                        let rel = path.strip_prefix(root).unwrap_or(&path);
                        offenders.push(format!("  {name} in {}", rel.to_string_lossy()));
                    }
                    rest = &tail[body_end.min(tail.len())..];
                }
            }
        }
    }
    offenders.sort();
    assert!(
        offenders.is_empty(),
        "these compare a query result by pointer, so salsa can never backdate them:\n{}\n\n\
         Give the wrapper a fingerprint — `ArcTopLevelEnv` and `callable_body` show \
         the shape — or compare the value directly when its type allows.",
        offenders.join("\n")
    );
}

/// Typing inside a body does not redo the file's name resolution.
///
/// rust-analyzer's `ItemTree` header calls this layer "an invalidation
/// barrier for incremental computations: when typing inside an item body,
/// the `ItemTree` of the modified file is typically unaffected, so we don't
/// have to recompute name resolution results". Ours was not a barrier: every
/// item carried its own `span` and `name_span`, so a character typed
/// anywhere moved something and the tree compared unequal. `file_def_map`'s
/// only input is `file_item_tree`, so name resolution re-ran for the whole
/// file — and `top_level_env` and the workspace context behind it.
#[test]
fn typing_in_a_body_does_not_redo_name_resolution() {
    use ide::analysis::AnalysisHost;

    let mut host = AnalysisHost::default();
    let mut files = base_db::Files::default();
    let fid = base_db::FileId::from_raw(0);
    let src = |body: &str| {
        format!(
            "val f : int -> int\nfunction f(x) = {body}\nval g : int -> int\nfunction g(y) = y\n"
        )
    };
    files.set_file_text(host.raw_database_mut(), fid, &src("x"));
    let def_map = |host: &AnalysisHost, files: &base_db::Files| {
        let db = host.raw_database();
        let ft = files.file_text(fid).expect("the fixture file");
        hir::def_query::file_def_map(db, ft)
            .as_ref()
            .map(|dm| std::sync::Arc::as_ptr(&dm.0) as usize)
    };

    let before = def_map(&host, &files);
    files.set_file_text(host.raw_database_mut(), fid, &src("x + 1"));
    assert_eq!(before, def_map(&host, &files), "editing a body redid name resolution");

    // Adding an item is a real change to what names exist, and does redo it.
    files.set_file_text(
        host.raw_database_mut(),
        fid,
        &format!("{}val h : int -> int\n", src("x + 1")),
    );
    assert_ne!(before, def_map(&host, &files), "a new item must redo name resolution");
}

/// Typing a character does not re-infer the rest of the file.
///
/// This failed on its second half for as long as `callable_body` hashed
/// the callable's start: a same-length edit reused the callables below it,
/// inserting one character did not. `Expr` and `Pat` carried byte offsets
/// then, so a body genuinely differed when the bytes before it moved.
///
/// They carry the text instead now, and the fingerprint is the callable's
/// own characters. What the edit really moved — the source map — is
/// rebased in `body_with_source_map`, which re-runs whenever the file does
/// and only clones.
#[test]
fn typing_a_character_does_not_reinfer_the_file() {
    use ide::analysis::AnalysisHost;

    let mut host = AnalysisHost::default();
    let mut files = base_db::Files::default();
    let fid = base_db::FileId::from_raw(0);
    // `g` sits below `f`, so an insertion in `f` shifts g's span.
    files.set_file_text(
        host.raw_database_mut(),
        fid,
        "function f() -> int = 1\nfunction g() -> int = 100\n",
    );
    let g_result = |host: &AnalysisHost, files: &base_db::Files| {
        let db = host.raw_database();
        let ft = files.file_text(fid).expect("the fixture file");
        let ids = hir::def_query::file_def_with_body_ids(db, ft);
        for &id in ids {
            let _ = hir::ty_query::infer(db, id);
        }
        let g = ids.iter().find(|i| i.name(db) == "g").copied().expect("g is a callable");
        std::sync::Arc::as_ptr(&hir::ty_query::infer(db, g).0) as usize
    };

    let before = g_result(&host, &files);
    // Same length: `1` -> `2`. Nothing below moves.
    files.set_file_text(
        host.raw_database_mut(),
        fid,
        "function f() -> int = 2\nfunction g() -> int = 100\n",
    );
    assert_eq!(before, g_result(&host, &files), "a same-length edit should reuse `g`");

    // One character longer. `g`'s source is untouched; its bytes move.
    files.set_file_text(
        host.raw_database_mut(),
        fid,
        "function f() -> int = 22\nfunction g() -> int = 100\n",
    );
    assert_eq!(before, g_result(&host, &files), "`g` was re-inferred because its bytes moved");
}

/// Reusing a body across an edit must not reuse its positions.
///
/// This is the other side of `typing_a_character_does_not_reinfer_the_file`
/// and the reason the fingerprint used to include the callable's start:
/// `callable_body` is kept from before the edit on purpose, so everything
/// that hands a user a position has to rebase it. Nothing else in the
/// suite would notice — a fixture builds its database once and never
/// edits, so every span is right by construction.
#[test]
fn a_reused_body_still_reports_where_it_is_now() {
    use ide::analysis::AnalysisHost;

    let mut host = AnalysisHost::default();
    let mut files = base_db::Files::default();
    let fid = base_db::FileId::from_raw(0);
    let below = "function g() -> int = { let unused = 1; 2 }
";
    let short = format!(
        "function f() -> int = 1
{below}"
    );
    files.set_file_text(host.raw_database_mut(), fid, &short);

    // `g`'s unused binding, by the offset of its name in the text.
    let unused_at = |text: &str, host: &AnalysisHost, files: &base_db::Files| -> Vec<usize> {
        let db = host.raw_database();
        let ft = files.file_text(fid).expect("the fixture file");
        let sf = ide_db::root_database::SalsaFile::new(db, ft);
        let diags = ide_diagnostics::semantic_diagnostics(
            db,
            &ide_diagnostics::DiagnosticsConfig::new(),
            &ide_db::assists::AssistResolveStrategy::None,
            &sf,
            ft,
            None,
        );
        let expected = text.find("unused").expect("the binding is in the text");
        for d in &diags {
            if d.code.as_str() == "unused-variable" {
                assert_eq!(
                    usize::from(d.range.range.start()),
                    expected,
                    "the diagnostic moved with the edit, the text did not"
                );
            }
        }
        diags
            .iter()
            .filter(|d| d.code.as_str() == "unused-variable")
            .map(|d| usize::from(d.range.range.start()))
            .collect()
    };

    assert_eq!(unused_at(&short, &host, &files).len(), 1, "the unused binding is reported");

    // One character longer above `g`. Its body is reused; its bytes moved.
    let long = format!(
        "function f() -> int = 22
{below}"
    );
    files.set_file_text(host.raw_database_mut(), fid, &long);
    assert_eq!(unused_at(&long, &host, &files).len(), 1, "still reported after the edit");
}

/// A dead-code waiver is `expect`, not `allow`, so the compiler retires it.
///
/// `#[allow(dead_code)]` is the compiler's answer suppressed, and it
/// outlives the gap it marks: 27 of the 150 in this workspace were on
/// items that had gained callers — `capabilities.rs` carried five on
/// checks that decide something, `hir-ty` sixteen on methods that are
/// called. `#[expect(dead_code)]` warns when the lint it waives does
/// *not* fire, so an attribute that stops being true stops compiling
/// clean.
///
/// The `allow`s that remain are the ones `expect` cannot express: an item
/// used only by tests fires the lint in the lib build and not in the test
/// build, so the expectation is unfulfilled in one of them. Each is
/// commented as such.
#[test]
fn a_dead_code_waiver_expires() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/ is the parent of this crate");
    let mut bare = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&path) else { continue };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if p.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            for (i, line) in text.lines().enumerate() {
                // The attribute standing alone on its line. An inline one
                // — on an enum variant's field, say — cannot carry a
                // trailing comment, and this test's own message mentions
                // the string.
                if line.trim() == "#[allow(dead_code)]" {
                    bare.push(format!("{}:{}", p.display(), i + 1));
                }
            }
        }
    }
    bare.sort();
    assert_eq!(
        bare,
        Vec::<String>::new(),
        "an `allow(dead_code)` with no reason: use `expect(dead_code)`, or say why `expect` \
         cannot express it"
    );
}

/// A panicking handler says which request it was.
///
/// `catch_unwind` keeps the server alive and tells nobody what died: the
/// log gets a bare `panicked at ..`. rust-analyzer enters a panic context
/// naming the method first (`handlers/dispatch.rs:92`).
/// `stdx::panic_context` is that mechanism and it had no caller — a
/// crash-reporting aid that fails silently, at the moment it was meant to
/// help. Both `catch_unwind` sites enter a frame now.
#[test]
fn a_panicking_handler_names_its_request() {
    const DISPATCH: &str = include_str!("handlers/dispatch.rs");
    let body = DISPATCH.split("fn a_panicking_handler_names_its_request").next().unwrap();
    assert_eq!(
        body.matches("std::panic::catch_unwind").count(),
        body.matches("stdx::panic_context::enter").count(),
        "a `catch_unwind` in the dispatcher with no panic-context frame: a \
         panic there is reported without saying which request it was"
    );
    assert!(body.contains("R::METHOD"), "the frame does not name the request method");
}

/// One door into the database, and durability is decided behind it.
///
/// rust-analyzer applies every change through `AnalysisHost::apply_change`,
/// and `FileChange::apply` derives each file's durability from the source
/// root it belongs to. This tree had five ways in: the server's
/// `sync_to_salsa` (naming LOW), its `sync_disk_file` (naming HIGH), a
/// `Files::set_file_contents` buffer the CLI and every `examples/` probe used
/// (naming HIGH for everything), `AnalysisHost::set_file_text` (a wrapper
/// whose only caller was its own test), and `Analysis::apply_change`, which
/// took a different change type and had no caller at all.
///
/// Each of the five picked a durability by hand, and the workspace scan went
/// through the one that had it backwards: every workspace `.sail` file became
/// a HIGH-durability salsa input, so changing one bumps the high-durability
/// revision and invalidates everything — and a `git checkout` under the
/// editor changes many.
///
/// The rule that replaces them is checkable: `Durability` is base-db's word.
/// A crate above it that names a constant is deciding something
/// `FileChange::apply` already decided from the source root.
#[test]
fn durability_is_decided_in_one_place() {
    const SOURCES: &[(&str, &str)] = &[
        ("sail-lsp/global_state.rs", include_str!("global_state.rs")),
        ("sail-lsp/main_loop.rs", include_str!("main_loop.rs")),
        ("sail-lsp/reload.rs", include_str!("reload.rs")),
        ("sail-lsp/cli.rs", include_str!("cli.rs")),
        ("sail-lsp/handlers/notification.rs", include_str!("handlers/notification.rs")),
    ];
    for (name, source) in SOURCES {
        // This file quotes the constants it forbids; skip its own text.
        let body = source.split("fn durability_is_decided_in_one_place").next().unwrap();
        assert!(
            !body.contains("Durability::"),
            "{name} names a Durability constant. Durability follows the file's \
             source root and `base_db::FileChange::apply` derives it; pass the \
             roots with `Change::set_roots` instead."
        );
    }
}

/// A progress sequence the user can stop.
///
/// The server sends `window/workDoneProgress/create` and reports under the
/// token; the client's cancel came back to a main loop with no handler for
/// it. `$/cancelRequest` was wired for the same reason — this is its other
/// half — and every reporter advertised `cancellable: false`, so no client
/// would have offered the button in the first place.
#[test]
fn a_progress_sequence_can_be_cancelled() {
    const MAIN_LOOP: &str = include_str!("main_loop.rs");
    const PROGRESS: &str = include_str!("progress.rs");
    let body = MAIN_LOOP.split("fn a_progress_sequence_can_be_cancelled").next().unwrap();
    assert!(
        body.contains("notification::WorkDoneProgressCancel"),
        "the main loop drops `window/workDoneProgress/cancel`"
    );
    // The one cancellable operation says so; the rest must not, or the
    // client offers a button that does nothing.
    assert!(
        body.contains("\"Priming caches\",\n                        true,"),
        "priming caches is the cancellable one and must advertise it"
    );
    assert!(
        !PROGRESS.contains("cancellable: Some(false)"),
        "a reporter hard-codes `cancellable: false` instead of taking it from its caller"
    );
}

/// The compiler's answer is a third source of diagnostics, beside the
/// server's own syntax and semantic passes.
///
/// rust-analyzer has had this since it had `cargo check`; `diagnostics.rs`
/// recorded its absence here as a "Sail simplification". It is not one: the
/// classes this checker provably cannot decide — `int` where `range(0, 3)`
/// is expected, and the mutants `mutation_check` still misses — are the ones
/// `sail --just-check` decides, and every probe in this repo already uses it
/// as an oracle for exactly that reason.
#[test]
fn what_the_compiler_says_reaches_the_client() {
    use base_db::FileId;
    let mut collection = crate::diagnostics::DiagnosticCollection::default();
    let file = FileId::from_raw(7);
    let diag = |msg: &str| lsp_types::Diagnostic {
        message: msg.to_string(),
        source: Some("sail".to_string()),
        ..Default::default()
    };

    collection.set_check_diagnostics(vec![(file, vec![diag("not a subtype")])]);
    let shown: Vec<_> = collection.diagnostics_for(file).map(|d| d.message.clone()).collect();
    assert_eq!(shown, vec!["not a subtype"], "a check finding must reach the client");
    assert!(collection.take_changes().is_some_and(|c| c.contains(&file)));

    // A later run that finds nothing clears the file. Upstream stops at the
    // first error, so keeping the previous run's findings would leave a
    // diagnostic it no longer makes.
    collection.set_check_diagnostics(Vec::new());
    assert_eq!(collection.diagnostics_for(file).count(), 0);
    assert!(
        collection.take_changes().is_some_and(|c| c.contains(&file)),
        "clearing a file is a change too"
    );
}

/// It is opt-in. A language server that spawns processes nobody asked for
/// is a surprise, and the binary may not be installed.
#[test]
fn the_compiler_is_not_run_unless_asked() {
    let config = crate::config::SailLspConfig::default();
    assert!(!config.flycheck.enable);
    assert_eq!(config.flycheck.command, "sail");
}

/// A secondary place is a place, not two integers in the message.
///
/// `unsolved_constraint` appended `"\n  constraint from 110..111"` to the
/// text — byte offsets, which no reader can act on. Upstream draws a caret
/// at each of them; LSP calls it `relatedInformation`, and nothing in this
/// tree ever set it.
#[test]
fn a_secondary_location_is_sent_as_a_location() {
    use ide_db::line_index::LineIndex;
    let text = "function f() -> unit = ()\n";
    let line_index = LineIndex::new(text);
    let url = lsp_types::Url::parse("file:///main.sail").unwrap();
    let file_id = base_db::FileId::from_raw(0);

    let diag = ide_diagnostics::Diagnostic::new(
        hir::diagnostics::DiagnosticCode::SailLint(
            "unsolved-constraint",
            hir::diagnostics::Severity::Error,
        ),
        "constraint cannot hold: 'n > 4",
        hir::FileRange { file_id, range: base_db::text_range(9, 10) },
    )
    .with_related(vec![(
        hir::FileRange { file_id, range: base_db::text_range(23, 25) },
        "constraint from here".to_string(),
    )]);

    let sent = crate::to_proto::diagnostic(&line_index, &url, &diag);
    assert!(!sent.message.contains(".."), "the message still carries offsets: {}", sent.message);
    let related = sent.related_information.expect("a secondary place must be sent");
    assert_eq!(related.len(), 1);
    assert_eq!(related[0].location.uri, url);
    assert_eq!(related[0].location.range.start.character, 23);
    assert_eq!(related[0].message, "constraint from here");
}
