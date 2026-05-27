//! Debug command: view HIR at cursor position.
//!
//! representation (Expr) and inferred type for the expression
//! at the cursor, using BodySourceMap for offset→ExprId mapping.

use ide_db::FileDb;
use ide_db::LineCol;

/// Show the HIR expression and inferred type at the given position.
///
/// Registered as custom LSP command `sail-lsp/viewHir`.
pub fn view_hir_at(file: &dyn FileDb, position: LineCol) -> String {
    let offset = file.offset_at(&position);
    let text = file.text();
    if text.is_empty() {
        return "(empty file)".to_string();
    }

    let (cst_root, _) = syntax::parse_text(text);
    let bodies = hir_def::bodies::CallableBodies::from_cst(&cst_root);

    let mut result = Vec::new();

    for entry in bodies.entries() {
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

            // Type info: run inline inference on body
            // (check_file takes SourceFileInfo, use text-based adapter)
            struct TextAdapter<'a>(&'a str);
            impl hir_def::callgraph::WorkspaceFile for TextAdapter<'_> {
                fn content_hash(&self) -> u64 {
                    0
                }
                fn callgraph(&self) -> Option<&hir_def::callgraph::CallGraph> {
                    None
                }
            }
            impl hir_def::callgraph::SourceFileInfo for TextAdapter<'_> {
                fn text(&self) -> &str {
                    self.0
                }
                fn item_tree(&self) -> Option<&hir_def::ItemTree> {
                    None
                }
            }
            let adapter = TextAdapter(text);
            let check_result = hir_ty::infer::check_file(&adapter);
            if let Some(ref tc) = check_result {
                if let Some(ty) = tc.type_of_expr.get(expr_id) {
                    result.push(format!("Type: {:?}", ty));
                } else {
                    result.push("Type: (not inferred for this ExprId)".to_string());
                }
            } else {
                result.push("Type: (typecheck unavailable)".to_string());
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

    #[test]
    fn view_hir_in_function_body() {
        let file = TestFile::new("function f(x : int) -> int = x + 1\n");
        let output = view_hir_at(&file, LineCol { line: 0, col: 30 });
        assert!(
            output.contains("In callable: f") || output.contains("Expr"),
            "should show HIR info, got: {output}"
        );
    }

    #[test]
    fn view_hir_outside_body() {
        let file = TestFile::new("val x : int\n");
        let output = view_hir_at(&file, LineCol { line: 0, col: 0 });
        assert!(output.contains("not inside"));
    }
}
