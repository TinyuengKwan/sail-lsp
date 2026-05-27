//! Handler for `AnyDiagnostic::DuplicateDefinition`.
//! Fix: rename the duplicate or remove it.

use crate::Diagnostic;
use hir_def::diagnostics::DiagnosticCode;
use ide_db::assists::Assist;
use ide_db::source_change::SourceChange;
use ide_db::text_edit::TextEdit;

use crate::{DiagnosticsContext, DuplicateDefinition};

pub(crate) fn duplicate_definition(
    _ctx: &DiagnosticsContext<'_>,
    d: &DuplicateDefinition,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError("duplicate-definition"),
        format!("duplicate definition of `{}`", d.name),
        crate::node_file_range(&d.node),
    )
    .with_fixes(fixes(d))
}

/// Fix: rename the duplicate definition by appending `_2` suffix.
fn fixes(d: &DuplicateDefinition) -> Option<Vec<Assist>> {
    let range: ide_db::line_index::TextRange = d.node.value.text_range().into();
    let new_name = format!("{}_2", d.name);
    let edit = TextEdit { range, new_text: new_name.clone() };
    Some(vec![crate::fix(
        "rename_duplicate",
        &format!("Rename to `{new_name}`"),
        SourceChange::from_text_edit(edit),
        range,
    )])
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::DuplicateDefinition(DuplicateDefinition {
            name: "Foo".to_string(),
            node: test_node(0, 5),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("duplicate definition of `Foo`"));
    }
}
