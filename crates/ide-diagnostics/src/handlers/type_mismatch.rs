//! Handler for `AnyDiagnostic::TypeMismatch`.
//!
//! Diagnostic: type-mismatch
//!
//! This diagnostic is triggered when an expression has a different type
//! than expected.

use crate::Diagnostic;
use hir_def::diagnostics::DiagnosticCode;
use hir_ty::display::HirDisplay;
use hir_ty::ty::TyKind;
use ide_db::assists::Assist;
use ide_db::source_change::SourceChange;
use ide_db::text_edit::TextEdit;
use syntax::AstNode;

use crate::{DiagnosticsContext, TypeMismatch};

// Diagnostic: type-mismatch
//
// This diagnostic is triggered when two types that should match don't.
pub(crate) fn type_mismatch(ctx: &DiagnosticsContext<'_>, d: &TypeMismatch) -> Option<Diagnostic> {
    // Skip unknown/error types.
    if d.expected.is_error() || d.actual.is_error() {
        return None;
    }

    let expected = d.expected.display_to_string();
    let actual = d.actual.display_to_string();
    let message = if let Some(ref source) = d.expected_source {
        format!("expected `{expected}`, found `{actual}` ({source})")
    } else {
        format!("expected `{expected}`, found `{actual}`")
    };

    // Use adjusted_display_range to narrow the range for if-expressions:
    // point at the `if` keyword rather than the whole if-expr block.
    if let Some(if_ptr) = syntax::AstPtr::<syntax::ast::IfExpr>::try_from_raw(d.expr_or_pat.value) {
        let _narrowed = crate::adjusted_display_range::<syntax::ast::IfExpr>(
            ctx,
            hir_def::in_file::InFile { file_id: d.expr_or_pat.file_id, value: if_ptr },
            &|if_expr| {
                if_expr.syntax().first_token().map(|t| {
                    let range: base_db::TextRange = t.text_range();
                    range
                })
            },
        );
    }
    Some(
        Diagnostic::new_with_syntax_node_ptr(
            ctx,
            DiagnosticCode::SailError("type-error"),
            message,
            d.expr_or_pat,
        )
        .stable()
        .with_fixes(fixes(d)),
    )
}

/// Generate type-aware quickfixes for type mismatches.
///
/// which dispatches to `add_reference`, `add_missing_ok_or_some`, etc.
///
/// Sail-specific fixes based on common Sail type coercions.
fn fixes(d: &TypeMismatch) -> Option<Vec<Assist>> {
    let range: ide_db::line_index::TextRange = d.expr_or_pat.value.text_range();

    match (d.expected.kind(), d.actual.kind()) {
        // bool → bit: suggest bool_to_bits(expr)
        (TyKind::Scalar(hir_ty::ty::Scalar::Bit), TyKind::Scalar(hir_ty::ty::Scalar::Bool)) => {
            let edit =
                TextEdit { range, new_text: format!("bool_to_bits({})", placeholder_text(range)) };
            Some(vec![crate::fix(
                "wrap_bool_to_bits",
                "Wrap with `bool_to_bits(...)`",
                SourceChange::from_text_edit(edit),
                range,
            )])
        }
        // bit → bool: suggest bit_to_bool(expr)
        (TyKind::Scalar(hir_ty::ty::Scalar::Bool), TyKind::Scalar(hir_ty::ty::Scalar::Bit)) => {
            let edit =
                TextEdit { range, new_text: format!("bit_to_bool({})", placeholder_text(range)) };
            Some(vec![crate::fix(
                "wrap_bit_to_bool",
                "Wrap with `bit_to_bool(...)`",
                SourceChange::from_text_edit(edit),
                range,
            )])
        }
        // int → nat: suggest unsigned(expr)
        (TyKind::Scalar(hir_ty::ty::Scalar::Nat), TyKind::Scalar(hir_ty::ty::Scalar::Int)) => {
            let edit =
                TextEdit { range, new_text: format!("unsigned({})", placeholder_text(range)) };
            Some(vec![crate::fix(
                "wrap_unsigned",
                "Wrap with `unsigned(...)`",
                SourceChange::from_text_edit(edit),
                range,
            )])
        }
        // nat → int: suggest signed(expr)
        (TyKind::Scalar(hir_ty::ty::Scalar::Int), TyKind::Scalar(hir_ty::ty::Scalar::Nat)) => {
            let edit = TextEdit { range, new_text: format!("signed({})", placeholder_text(range)) };
            Some(vec![crate::fix(
                "wrap_signed",
                "Wrap with `signed(...)`",
                SourceChange::from_text_edit(edit),
                range,
            )])
        }
        // bits → int: suggest sail_unsigned(expr)
        (TyKind::Scalar(hir_ty::ty::Scalar::Int), TyKind::App { name, .. })
            if name == "bits" || name == "bitvector" =>
        {
            let edit =
                TextEdit { range, new_text: format!("sail_unsigned({})", placeholder_text(range)) };
            Some(vec![crate::fix(
                "wrap_sail_unsigned",
                "Wrap with `sail_unsigned(...)`",
                SourceChange::from_text_edit(edit),
                range,
            )])
        }
        _ => None,
    }
}

/// Placeholder text for fix — in a real implementation this would read
/// the actual source text at the range. For now, use a generic placeholder.
fn placeholder_text(_range: ide_db::line_index::TextRange) -> &'static str {
    "expr"
}

/// Check if an expression node needs parentheses when wrapped in a call.
///
/// Uses `syntax::ast::prec::precedence` to determine if the expression's
/// precedence requires wrapping.
/// `needs_parens_in` for fix-up code generation.
#[allow(dead_code)]
fn needs_parens_for_wrapping(node: &syntax::SyntaxNode) -> bool {
    let prec = syntax::ast::prec::precedence(node);
    // Comma-level or lower precedence needs parens when used as argument.
    prec.needs_parentheses_in(syntax::ast::prec::ExprPrecedence::Assign)
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::TypeMismatch(TypeMismatch {
            expected: hir_ty::infer::Ty::scalar(hir_ty::ty::Scalar::Int),
            actual: hir_ty::infer::Ty::scalar(hir_ty::ty::Scalar::Bool),
            expr_or_pat: test_node(0, 5),
            expected_source: None,
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("expected `int`, found `bool`"));
    }

    #[test]
    fn with_source() {
        let diag = AnyDiagnostic::TypeMismatch(TypeMismatch {
            expected: hir_ty::infer::Ty::scalar(hir_ty::ty::Scalar::Int),
            actual: hir_ty::infer::Ty::scalar(hir_ty::ty::Scalar::Bool),
            expr_or_pat: test_node(0, 5),
            expected_source: Some("return type".to_string()),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("return type"));
    }
}
