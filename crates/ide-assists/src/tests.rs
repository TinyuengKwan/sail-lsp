//! Test helpers for assist handler tests.
//! Provides `check_assist`, `check_assist_not_applicable`, and
//! `check_assist_label` helpers used by every handler's `#[cfg(test)] mod tests`.

use crate::assist_context::{AssistContext, Assists, Handler};
use ide_db::test_utils::TestFile;

/// Check that an assist produces at least one result with a non-empty label.
///
/// to check that the handler fires and returns a labeled assist.
///
/// `cursor` specifies the byte offset for the cursor position.
pub(crate) fn check_assist(handler: Handler, source: &str, cursor: usize) -> Vec<String> {
    let file = TestFile::new(source);
    let range = base_db::text_range(cursor, cursor);
    let ctx = AssistContext::new(&file, range);
    let mut acc = Assists::new();
    handler(&mut acc, &ctx);
    let assists = acc.finish();
    assists.iter().map(|a| a.label.to_string()).collect()
}

/// Check that an assist handler fires and returns at least one assist.
#[allow(dead_code)]
pub(crate) fn check_assist_applicable(handler: Handler, source: &str, cursor: usize) {
    let labels = check_assist(handler, source, cursor);
    assert!(!labels.is_empty(), "expected assist to be applicable at offset {cursor}");
}

/// Check that an assist handler does NOT fire.
#[allow(dead_code)]
pub(crate) fn check_assist_not_applicable(handler: Handler, source: &str, cursor: usize) {
    let labels = check_assist(handler, source, cursor);
    assert!(
        labels.is_empty(),
        "expected assist to NOT be applicable at offset {cursor}, got: {labels:?}"
    );
}

/// Check that an assist handler produces an assist with the expected label.
#[allow(dead_code)]
pub(crate) fn check_assist_label(
    handler: Handler,
    source: &str,
    cursor: usize,
    expected_label: &str,
) {
    let labels = check_assist(handler, source, cursor);
    assert!(
        labels.iter().any(|l| l == expected_label),
        "expected assist with label `{expected_label}`, got: {labels:?}"
    );
}

/// Run an assist and return the labels as a newline-joined string.
///
/// Useful with `expect_test::expect!` for snapshot testing.
#[allow(dead_code)]
pub(crate) fn check_assist_snapshot(handler: Handler, source: &str, cursor: usize) -> String {
    let labels = check_assist(handler, source, cursor);
    if labels.is_empty() {
        "(no assist)".to_string()
    } else {
        labels.join("\n")
    }
}
