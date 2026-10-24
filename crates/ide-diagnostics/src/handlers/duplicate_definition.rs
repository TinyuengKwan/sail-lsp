//! Handler for `AnyDiagnostic::DuplicateDefinition`.
//! Fix: rename the duplicate or remove it.

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;
use ide_db::assists::Assist;
use ide_db::source_change::SourceChange;
use ide_db::text_edit::TextEdit;

use crate::{DiagnosticsContext, DuplicateDefinition};

pub(crate) fn duplicate_definition(
    _ctx: &DiagnosticsContext<'_>,
    d: &DuplicateDefinition,
) -> Diagnostic {
    // `already_bound` (`type_env.ml:420-434`): the caret is on the name
    // the second definition declares, and the first binding gets a second
    // caret hinted "previous binding". This said "duplicate definition of
    // `foo1`", which names neither what is being created nor where the
    // name came from — six of `fail/`'s cases.
    //
    // A type synonym is a different message upstream, because a synonym
    // may legitimately be redeclared in another module and the check that
    // rejects it is elsewhere (`Cannot define type synonym foo, as a type
    // or synonym with that name already exists`).
    let message = if d.creating == "type synonym" {
        format!(
            "Cannot define type synonym {}, as a type or synonym with that name already exists",
            d.name
        )
    } else {
        format!("Cannot create {} {}, name is already bound", d.creating, d.name)
    };
    let related = d
        .first
        .as_ref()
        .map(|f| vec![(crate::node_file_range(f), "previous binding".to_string())])
        .unwrap_or_default();
    Diagnostic::new(
        DiagnosticCode::SailError("duplicate-definition"),
        message,
        crate::node_file_range(&d.node),
    )
    .with_related(related)
    .with_fixes(fixes(d))
}

/// Fix: rename the duplicate definition by appending `_2` suffix.
fn fixes(d: &DuplicateDefinition) -> Option<Vec<Assist>> {
    let range: ide_db::line_index::TextRange = d.node.value.text_range();
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
            creating: "union type",
            first: None,
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("Cannot create union type Foo, name is already bound"));
    }
}
