//! Handler for `AnyDiagnostic::IncompleteMatch`.
//!
//! Diagnostic: missing-match-arm
//!
//! This diagnostic is triggered if `match` block is missing one or more
//! match arms.
//!
//! Fix : generate all missing variant arms when known,
//! fall back to wildcard arm.

use crate::Diagnostic;
use hir_def::diagnostics::{DiagnosticCode, Severity};
use ide_db::source_change::SourceChange;
use ide_db::text_edit::TextEdit;

use crate::{DiagnosticsContext, IncompleteMatch};

// Diagnostic: missing-match-arm
//
// This diagnostic is triggered if `match` block is missing one or more match arms.
pub(crate) fn missing_match_arms(_ctx: &DiagnosticsContext<'_>, d: &IncompleteMatch) -> Diagnostic {
    let message = if d.missing_arms.is_empty() {
        "non-exhaustive match".to_string()
    } else {
        format!("non-exhaustive match: missing {}", d.missing_arms.join(", "))
    };
    Diagnostic::new(
        DiagnosticCode::SailLint("incomplete-match", Severity::Warning),
        message,
        crate::node_file_range(&d.node),
    )
    .with_fixes(fixes(d))
}

/// Generate fixes for incomplete match.
///
/// - If missing arms are known, generate an arm for each variant.
/// - Otherwise, add a wildcard arm `_ => ()`.
fn fixes(d: &IncompleteMatch) -> Option<Vec<ide_db::assists::Assist>> {
    let range: ide_db::line_index::TextRange = d.node.value.text_range().into();
    let insert_range = base_db::text_range(base_db::range_end(range), base_db::range_end(range));

    let mut assists = Vec::new();

    // Fix 1: If we know the missing arms, generate them all.
    if !d.missing_arms.is_empty() {
        let arms_text = d
            .missing_arms
            .iter()
            .map(|arm| format!("    {arm} => (),"))
            .collect::<Vec<_>>()
            .join("\n");
        let edit = TextEdit { range: insert_range, new_text: format!("\n{arms_text}\n") };
        assists.push(crate::fix(
            "add_missing_match_arms",
            "Add missing match arms",
            SourceChange::from_text_edit(edit),
            insert_range,
        ));
    }

    // Fix 2: Always offer wildcard arm as alternative.
    let wildcard_edit = TextEdit { range: insert_range, new_text: "\n    _ => (),\n".to_string() };
    assists.push(crate::fix(
        "add_wildcard_arm",
        "Add wildcard arm `_ => ()`",
        SourceChange::from_text_edit(wildcard_edit),
        insert_range,
    ));

    Some(assists)
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::IncompleteMatch(IncompleteMatch {
            missing_arms: vec!["Foo".to_string(), "Bar".to_string()],
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("non-exhaustive match: missing Foo, Bar"));
    }

    #[test]
    fn empty_arms() {
        let diag = AnyDiagnostic::IncompleteMatch(IncompleteMatch {
            missing_arms: vec![],
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert_eq!(d.unwrap().message, "non-exhaustive match");
    }
}
