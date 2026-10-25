//! Fixes a diagnostic's own handler builds, as an editor receives them:
//! through `Analysis::code_actions`, for the diagnostics the client sends
//! back.

use crate::fixture;

/// Apply every code action offered for a diagnostic of `code` in the first
/// fixture file and return `(title, resulting text)` for each.
#[track_caller]
fn fixes_for(fixture: &str, code: &str) -> Vec<(String, String)> {
    let (analysis, files) = fixture::analysis(fixture);
    let (file_id, _) = files[0];
    let url = analysis.url_for_file_id(file_id).expect("fixture file has a URL").clone();
    let text = analysis
        .files_ref()
        .file_text(file_id)
        .map(|ft| ft.text(analysis.db()).to_string())
        .unwrap();
    let diagnostics: Vec<_> = analysis
        .file_diagnostics(file_id, &ide_diagnostics::DiagnosticsConfig::new())
        .unwrap()
        .into_iter()
        .filter(|d| d.code.as_str() == code)
        .collect();
    assert!(!diagnostics.is_empty(), "no `{code}` diagnostic in the fixture");
    let actions = analysis
        .code_actions(
            &url,
            base_db::text_range(0, text.len()),
            &diagnostics,
            &Default::default(),
            &Default::default(),
        )
        .unwrap();
    actions
        .into_iter()
        .filter(|a| a.for_diagnostic.is_some())
        .map(|a| {
            assert_eq!(
                a.source_change.source_file_edits.keys().copied().collect::<Vec<_>>(),
                vec![file_id],
                "`{}` edits a file other than the diagnostic's",
                a.title
            );
            let mut edits = a.source_change.source_file_edits[&file_id].clone();
            edits.sort_by_key(|e| std::cmp::Reverse(base_db::range_start(e.range)));
            let mut out = text.clone();
            for e in edits {
                out.replace_range(
                    base_db::range_start(e.range)..base_db::range_end(e.range),
                    &e.new_text,
                );
            }
            (a.title, out)
        })
        .collect()
}

#[test]
fn a_type_mismatch_fix_wraps_the_expression_it_is_about() {
    // The prelude comes first, so a fix still naming the placeholder
    // `FileId(0)` would land in the library rather than here.
    let fixes = fixes_for(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
function f(v : bits(8), w : bits(8)) -> int = v + w
"#,
        "type-error",
    );
    let titles: Vec<_> = fixes.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(titles, ["Wrap with `unsigned(...)`", "Wrap with `signed(...)`"], "{fixes:#?}");
    assert!(fixes[0].1.contains("-> int = unsigned(v + w)\n"), "{}", fixes[0].1);
}

#[test]
fn no_fix_wraps_in_a_function_the_library_does_not_have() {
    let fixes = fixes_for(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
function f(b : bool) -> bit = b
"#,
        "type-error",
    );
    assert!(fixes.is_empty(), "{fixes:#?}");
}

#[test]
fn an_unused_variable_is_fixed_once() {
    // The quick fix and the handler's own fix make the same edit; the
    // client gets one action.
    let fixes = fixes_for(
        r#"
//- /main.sail
function f() -> int = { let x : int = 1; 2 }
"#,
        "unused-variable",
    );
    assert_eq!(fixes.len(), 1, "{fixes:#?}");
    assert!(fixes[0].1.contains("let _x : int"), "{}", fixes[0].1);
}
