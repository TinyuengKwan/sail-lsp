//! Handler for `AnyDiagnostic::TypeMismatch`.
//!
//! Diagnostic: type-mismatch
//!
//! This diagnostic is triggered when an expression has a different type
//! than expected.

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;
use hir::display::HirDisplay;
use ide_db::assists::Assist;
use ide_db::source_change::SourceChange;
use ide_db::text_edit::TextEdit;

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

    // Narrow the display only; fixes keep the complete expression pointer.
    let mut diagnostic = Diagnostic::new_with_syntax_node_ptr(
        ctx,
        DiagnosticCode::SailError("type-error"),
        message,
        d.expr_or_pat,
    )
    .stable()
    .with_fixes(fixes(ctx, d));
    if d.expr_or_pat.value.kind() == syntax::SyntaxKind::IF_EXPR {
        let keyword = ctx
            .file
            .filter(|file| file.file_id(ctx.db()) == d.expr_or_pat.file_id)
            .and_then(|file| ctx.sema.parse(file))
            .and_then(|root| d.expr_or_pat.value.try_to_node(&root))
            .and_then(|node| {
                node.children_with_tokens()
                    .filter_map(|it| it.into_token())
                    .find(|token| token.kind() == syntax::SyntaxKind::KW_IF)
            });
        if let Some(keyword) = keyword {
            diagnostic.range.range = keyword.text_range();
        }
    }
    Some(diagnostic)
}

/// Quick fixes for a type mismatch.
///
/// Only a conversion the library provides is offered: a bitvector where an
/// integer is expected, through `unsigned` or `signed` (both
/// `bits('n) -> range(..)`). The library has no `bool`/`bit` conversion and
/// no `int -> nat` coercion function, so there is nothing to wrap those in.
fn fixes(ctx: &DiagnosticsContext<'_>, d: &TypeMismatch) -> Option<Vec<Assist>> {
    if !(is_integer(&d.expected) && is_bitvector(&d.actual)) {
        return None;
    }
    let range: ide_db::line_index::TextRange = d.expr_or_pat.value.text_range();
    let file = ctx.file.filter(|file| file.file_id(ctx.db()) == d.expr_or_pat.file_id)?;
    let root = ctx.sema.parse(file)?;
    let text = d.expr_or_pat.value.try_to_node(&root)?.text().to_string();
    let wrap = |func: &'static str| {
        let edit = TextEdit { range, new_text: format!("{func}({text})") };
        crate::fix(
            func,
            &format!("Wrap with `{func}(...)`"),
            SourceChange::from_text_edit(edit),
            range,
        )
    };
    // Do not propose a call that is absent or shadowed by a local binding.
    // Constrained numeric destinations require a proof we do not have here.
    if d.expected.ctor_name() != Some("int") {
        return None;
    }
    let fixes = ["unsigned", "signed"].into_iter().filter(|name| {
        ctx.sema.callable_types_at(file, usize::from(range.start()), name).iter().any(|ty| {
            matches!(ty.kind(), hir::TyKind::FnPtr(sig) if sig.params.len() == 1 && is_bitvector(&sig.params[0]) && is_integer(&sig.ret))
        })
    }).map(wrap).collect::<Vec<_>>();
    (!fixes.is_empty()).then_some(fixes)
}

fn is_integer(ty: &hir::Ty) -> bool {
    matches!(ty.ctor_name(), Some("int" | "nat" | "atom" | "range" | "implicit"))
}

fn is_bitvector(ty: &hir::Ty) -> bool {
    matches!(ty.ctor_name(), Some("bits" | "bitvector"))
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

    #[test]
    fn if_diagnostic_highlights_keyword_but_keeps_the_full_fix_target() {
        use syntax::AstNode;
        let source = "val unsigned : bits(4) -> int\nfunction f() = if true then 0x1 else 0x2\n";
        let mut db = ide_db::root_database::RootDatabase::default();
        let id = base_db::FileId::from_raw(0);
        let mut change = ide_db::apply_change::Change::new();
        change.change_file(id, source);
        change.set_source_project(base_db::preprocess::SourceProject {
            roots: vec![id],
            paths: vec![("/main.sail".into(), id)],
            modules: base_db::module_graph::ModuleGraph { globals: vec![id], ..Default::default() },
            ..Default::default()
        });
        db.apply_change(change);
        let input = db.files().file_text(id).unwrap();
        let config = crate::DiagnosticsConfig::new();
        let mut ctx = crate::tests::test_ctx(&db, &config);
        ctx.file = Some(input);
        let root = ctx.sema.parse(input).unwrap();
        let node = root.descendants().find_map(syntax::ast::IfExpr::cast).unwrap();
        let ptr = syntax::SyntaxNodePtr::new(node.syntax());
        let diagnostic = super::type_mismatch(
            &ctx,
            &TypeMismatch {
                expected: hir::Ty::scalar(hir::Scalar::Int),
                actual: hir::Ty::named("bits"),
                expr_or_pat: hir::InFile::new(base_db::FileId::from_raw(0), ptr),
                expected_source: None,
            },
        )
        .unwrap();
        let range = diagnostic.range.range;
        assert_eq!(&source[usize::from(range.start())..usize::from(range.end())], "if");
        assert_eq!(diagnostic.main_node.unwrap().value.text_range(), ptr.text_range());
        let fix = diagnostic.fixes.unwrap().remove(0);
        assert_eq!(
            fix.source_change.unwrap().source_file_edits.values().next().unwrap()[0].range,
            ptr.text_range()
        );
    }

    #[test]
    fn synthetic_if_pointer_retains_its_range_without_panicking() {
        let db = ide_db::root_database::RootDatabase::default();
        let config = crate::DiagnosticsConfig::new();
        let ctx = crate::tests::test_ctx(&db, &config);
        let range = base_db::text_range(50, 90);
        let diagnostic = super::type_mismatch(
            &ctx,
            &TypeMismatch {
                expected: hir::Ty::scalar(hir::Scalar::Bit),
                actual: hir::Ty::scalar(hir::Scalar::Bool),
                expr_or_pat: hir::InFile::new(
                    base_db::FileId::from_raw(0),
                    syntax::SyntaxNodePtr::from_range(syntax::SyntaxKind::IF_EXPR, range),
                ),
                expected_source: None,
            },
        )
        .unwrap();
        assert_eq!(diagnostic.range.range, range);
    }

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
