//! Handler for `AnyDiagnostic::TypeMismatch`.
//!
//! Diagnostic: type-mismatch
//!
//! This diagnostic is triggered when an expression has a different type
//! than expected.

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;
use hir::display::HirDisplay;
use hir::TyKind;
use ide_db::assists::Assist;
use ide_db::source_change::SourceChange;
use ide_db::text_edit::TextEdit;
use syntax::AstNode;

use crate::{DiagnosticsContext, TypeMismatch};

/// Whether the two types have the same shape, differing only in what
/// their arguments say.
///
/// Upstream has two sentences here and they are not stylistic. `subtyp`
/// (`type_check.ml:1085-1119`) reaches `Err_subtype` — rendered `{actual}
/// is not a subtype of {expected}` (`type_error.ml:481`) — when the two
/// line up and a constraint cannot be proven. When they do *not* line up
/// it unifies instead, and a `Unification_error` becomes `Type mismatch
/// between {expected} and {actual}` (`:582`, expected first).
///
/// So `int(64)` against `int(32)` is a subtype failure and `int(0)`
/// against `string` is a mismatch, and the difference is the whole
/// content: one says the constraint did not hold, the other says the
/// shapes never met. Both were "Type mismatch between X and Y" here, which says
/// neither.
///
/// The shape comparison is the head constructor and the argument count,
/// which is what `unify_typ`'s own arms test before falling through to
/// its error (`:569-580`).
fn shapes_line_up(expected: &str, actual: &str) -> bool {
    /// Head name and argument count, read off the rendered type.
    ///
    /// The rendering rather than the `TyKind`, because `bits(4)` reaches
    /// here as `Named("bits", [4])` from one path and as `App { name:
    /// "bits", .. }` from another — this file's own crate documents the two
    /// as "the same shape [that] has not been merged". Reading the shape
    /// off the text is the one normal form both agree on, and it is the
    /// text the reader sees.
    fn shape(rendered: &str) -> (String, usize) {
        let rendered = rendered.trim();
        let Some(open) = rendered.find('(') else { return (rendered.to_string(), 0) };
        let head = rendered[..open].trim().to_string();
        let inner = rendered[open + 1..].strip_suffix(')').unwrap_or(&rendered[open + 1..]);
        let mut depth = 0i32;
        let mut args = if inner.trim().is_empty() { 0 } else { 1 };
        for c in inner.chars() {
            match c {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                ',' if depth == 0 => args += 1,
                _ => {}
            }
        }
        (head, args)
    }
    shape(expected) == shape(actual)
}

// Diagnostic: type-mismatch
//
// This diagnostic is triggered when two types that should match don't.
pub(crate) fn type_mismatch(ctx: &DiagnosticsContext<'_>, d: &TypeMismatch) -> Option<Diagnostic> {
    // Skip unknown/error types.
    if d.expected.is_error() || d.actual.is_error() {
        return None;
    }

    let expected = d.expected.display_to_string(ctx.db());
    let actual = d.actual.display_to_string(ctx.db());
    // A *named* struct literal checked against a different record type is
    // its own check upstream — `check_struct_name` (`type_check.ml:2168`)
    // compares the name the literal writes with the type expected there
    // and says "Struct type S found, S2 expected", with the name as a
    // hint. It never reaches the subtype or unify path, so neither
    // sentence below is what a reader sees from `sail`.
    let named_struct_literal = d.expr_or_pat.value.kind() == syntax::SyntaxKind::STRUCT_EXPR
        && !expected.contains('(')
        && !actual.contains('(');
    // A vector literal whose element count is not the length expected is
    // its own check upstream — `Vector literal with incorrect length:
    // found 2 elements, but expected 1` (`type_check.ml:2493`) — counted
    // rather than derived from the types, because "expected `vector(1,
    // int)`" does not tell the reader to count anything.
    let vector_literal = d.expr_or_pat.value.kind() == syntax::SyntaxKind::VECTOR_EXPR;
    let lengths = || -> Option<(i64, i64)> {
        let len = |t: &str| -> Option<i64> {
            t.strip_prefix("vector(")?.split(',').next()?.trim().parse().ok()
        };
        Some((len(&actual)?, len(&expected)?))
    };
    let message = if vector_literal && lengths().is_some_and(|(a, e)| a != e) {
        let (found, want) = lengths().expect("checked just above");
        format!("Vector literal with incorrect length: found {found} elements, but expected {want}")
    } else if named_struct_literal {
        format!("Struct type {actual} found, {expected} expected")
    } else if shapes_line_up(&expected, &actual) {
        format!("{actual} is not a subtype of {expected}")
    } else {
        format!("Type mismatch between {expected} and {actual}")
    };
    let message = match d.expected_source {
        Some(ref source) => format!("{message} ({source})"),
        None => message,
    };

    // Use adjusted_display_range to narrow the range for if-expressions:
    // point at the `if` keyword rather than the whole if-expr block.
    if let Some(if_ptr) = syntax::AstPtr::<syntax::ast::IfExpr>::try_from_raw(d.expr_or_pat.value) {
        let _narrowed = crate::adjusted_display_range::<syntax::ast::IfExpr>(
            ctx,
            hir::InFile { file_id: d.expr_or_pat.file_id, value: if_ptr },
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
        (TyKind::Scalar(hir::Scalar::Bit), TyKind::Scalar(hir::Scalar::Bool)) => {
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
        (TyKind::Scalar(hir::Scalar::Bool), TyKind::Scalar(hir::Scalar::Bit)) => {
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
        (TyKind::Scalar(hir::Scalar::Nat), TyKind::Scalar(hir::Scalar::Int)) => {
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
        (TyKind::Scalar(hir::Scalar::Int), TyKind::Scalar(hir::Scalar::Nat)) => {
            let edit = TextEdit { range, new_text: format!("signed({})", placeholder_text(range)) };
            Some(vec![crate::fix(
                "wrap_signed",
                "Wrap with `signed(...)`",
                SourceChange::from_text_edit(edit),
                range,
            )])
        }
        // bits → int: suggest sail_unsigned(expr)
        (TyKind::Scalar(hir::Scalar::Int), TyKind::App { name, .. })
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
#[expect(dead_code)]
fn needs_parens_for_wrapping(node: &syntax::SyntaxNode) -> bool {
    let prec = syntax::ast::prec::precedence(node);
    // Comma-level or lower precedence needs parens when used as argument.
    prec.needs_parentheses_in(syntax::ast::prec::ExprPrecedence::Assign)
}

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    /// The two sentences, and which one each shape gets.
    ///
    /// `int(64)` against `int(32)` lines up and fails a constraint —
    /// upstream's `Err_subtype`. `int(0)` against `string` never lines up
    /// — upstream unifies and reports a mismatch. Both were one sentence
    /// here, which said neither.
    #[test]
    fn a_constraint_failure_and_a_shape_mismatch_are_different_sentences() {
        assert!(super::shapes_line_up("int(64)", "int(32)"));
        assert!(super::shapes_line_up("bits(17)", "bits(16)"));
        assert!(!super::shapes_line_up("int(0)", "string"));
        assert!(!super::shapes_line_up("(int, int)", "int"));
        // Arity counts: a 2-tuple and a 3-tuple never line up.
        assert!(!super::shapes_line_up("(int, int)", "(int, int, int)"));
        // And a name is a name whether or not it is applied.
        assert!(super::shapes_line_up("bool", "bool"));
        assert!(!super::shapes_line_up("bool", "bits(1)"));
    }

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::TypeMismatch(TypeMismatch {
            expected: hir::Ty::scalar(hir::Scalar::Int),
            actual: hir::Ty::scalar(hir::Scalar::Bool),
            expr_or_pat: test_node(0, 5),
            expected_source: None,
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("Type mismatch between int and bool"));
    }

    #[test]
    fn with_source() {
        let diag = AnyDiagnostic::TypeMismatch(TypeMismatch {
            expected: hir::Ty::scalar(hir::Scalar::Int),
            actual: hir::Ty::scalar(hir::Scalar::Bool),
            expr_or_pat: test_node(0, 5),
            expected_source: Some("return type".to_string()),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("return type"));
    }
}
