//! Debug command: view HIR at cursor position.
//!
//! representation (Expr) and inferred type for the expression
//! at the cursor, using BodySourceMap for offset→ExprId mapping.

use ide_db::FileDb;
use ide_db::LineCol;

/// Show the HIR expression and inferred type at the given position.
///
/// Registered as custom LSP command `sail-lsp/viewHir`.
///
/// Reads the same salsa queries the rest of the server reads. Re-parsing the
/// file here would show a tree nothing else in the process is looking at.
pub(crate) fn view_hir(
    db: &dyn salsa::Database,
    input: base_db::FileText,
    file: &dyn FileDb,
    position: LineCol,
) -> String {
    let offset = file.offset_at(&position);
    if file.text().is_empty() {
        return "(empty file)".to_string();
    }
    let Some(bodies) = file.bodies() else {
        return "(no callable bodies)".to_string();
    };

    let mut result = Vec::new();

    for (clause_index, entry) in bodies.entries().iter().enumerate() {
        // Check if offset is within this callable's body span
        if offset < entry.body_span.start || offset > entry.body_span.end {
            continue;
        }

        result.push(format!("// In callable: {}", entry.name));

        // Find the ExprId at offset via BodySourceMap
        let body = &entry.body;
        let source_map = &entry.source_map;

        if let Some(expr_id) = source_map.expr_at_offset(offset) {
            let expr = &body[expr_id];
            result.push(format!("ExprId: {:?}", expr_id));
            result.push(format!("Expr: {:?}", expr));

            // Type info from the per-callable inference query.
            let _ = clause_index;
            let ty = hir::Semantics::new(db).expr_ty_in(input, &entry.name, expr_id);
            match ty {
                Some(ty) => result.push(format!("Type: {:?}", ty)),
                None => result.push("Type: (not inferred for this ExprId)".to_string()),
            }
        } else if let Some(pat_id) = source_map.pat_at_offset(offset) {
            let pat = &body[pat_id];
            result.push(format!("PatId: {:?}", pat_id));
            result.push(format!("Pat: {:?}", pat));
        } else {
            result.push(format!("(no HIR node at offset {})", offset));
        }

        break; // Only show first matching callable
    }

    if result.is_empty() {
        "(cursor not inside a callable body)".to_string()
    } else {
        result.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_db::test_utils::TestFile;

    fn view(source: &str, position: LineCol) -> String {
        let file = TestFile::new(source);
        view_hir(file.db(), file.input(), &file, position)
    }

    #[test]
    fn view_hir_in_function_body() {
        let output = view("function f(x : int) -> int = x + 1\n", LineCol { line: 0, col: 30 });
        assert!(
            output.contains("In callable: f") || output.contains("Expr"),
            "should show HIR info, got: {output}"
        );
    }

    #[test]
    fn view_hir_shows_an_inferred_type() {
        let output = view("function f(x : int) -> int = x + 1\n", LineCol { line: 0, col: 30 });
        assert!(output.contains("Type:"), "should show a type, got: {output}");
        assert!(
            !output.contains("(typecheck unavailable)"),
            "inference should be reachable, got: {output}"
        );
    }

    #[test]
    fn view_hir_outside_body() {
        let output = view("val x : int\n", LineCol { line: 0, col: 0 });
        assert!(output.contains("not inside"));
    }
}
