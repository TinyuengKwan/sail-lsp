//! Handler for `AnyDiagnostic::UnresolvedField`.

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;
use hir::display::HirDisplay;

use crate::{DiagnosticsContext, UnresolvedField};

pub(crate) fn unresolved_field(ctx: &DiagnosticsContext<'_>, d: &UnresolvedField) -> Diagnostic {
    let mut range = crate::node_file_range(&d.node);
    if !d.receiver_is_not_a_record {
        let field = ctx
            .file
            .filter(|file| file.file_id(ctx.db()) == d.node.file_id)
            .and_then(|file| ctx.sema.parse(file))
            .and_then(|root| d.node.value.try_to_node(&root))
            .filter(|node| node.kind() == syntax::SyntaxKind::FIELD_ACCESS_EXPR)
            .and_then(|node| {
                node.children_with_tokens().filter_map(|it| it.into_token()).find(|token| {
                    token.kind() == syntax::SyntaxKind::IDENT && token.text() == d.name
                })
            });
        if let Some(field) = field {
            range.range = field.text_range();
        }
    }
    Diagnostic::new(
        DiagnosticCode::SailError("unresolved-field"),
        // Two findings, and upstream words them apart. A record without
        // that field is "no field X on type Y"; a receiver that is not a
        // record at all is about the *type* — `Type accessed by field
        // expression is not a struct, it has type Foo`
        // (`type_check.ml:3678`), with the type as a hint at the
        // receiver. A union is the case the corpus has.
        if d.receiver_is_not_a_record && d.in_assign_target {
            "Type set using field l-expression must be a struct type".to_string()
        } else if d.receiver_is_not_a_record {
            format!(
                "Type accessed by field expression is not a struct, it has type {}",
                d.receiver.display_to_string(ctx.db())
            )
        } else {
            format!("no field `{}` on type `{}`", d.name, d.receiver.display_to_string(ctx.db()))
        },
        range,
    )
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::UnresolvedField(UnresolvedField {
            receiver: hir::Ty::scalar(hir::Scalar::Int),
            name: "bar".to_string(),
            node: test_node(0, 5),
            receiver_fields_known: true,
            receiver_is_not_a_record: false,
            in_assign_target: false,
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("no field `bar`"));
    }

    /// A receiver that is not a record at all is a different sentence,
    /// and a different one again on the left of an `=`.
    #[test]
    fn a_field_on_a_non_record_is_about_the_type() {
        let read = AnyDiagnostic::UnresolvedField(UnresolvedField {
            receiver: hir::Ty::named("Foo".to_string()),
            name: "B".to_string(),
            node: test_node(0, 3),
            receiver_fields_known: true,
            receiver_is_not_a_record: true,
            in_assign_target: false,
        });
        assert_eq!(
            check_dispatch(&read).map(|d| d.message),
            Some("Type accessed by field expression is not a struct, it has type Foo".to_string())
        );

        let write = AnyDiagnostic::UnresolvedField(UnresolvedField {
            receiver: hir::Ty::named("Foo".to_string()),
            name: "B".to_string(),
            node: test_node(0, 3),
            receiver_fields_known: true,
            receiver_is_not_a_record: true,
            in_assign_target: true,
        });
        assert_eq!(
            check_dispatch(&write).map(|d| d.message),
            Some("Type set using field l-expression must be a struct type".to_string())
        );
    }
}
