//! Body validation diagnostics — structural issues detected AFTER inference.
//! These diagnostics inspect the Body for patterns that are technically
//! valid but indicate likely bugs or style issues.
//!
//! Created as independent pass with immutable references.
//! Uses `ExprScopes` for unified binding enumeration.

use std::collections::HashSet;

use hir_def::expr_store::scope::ExprScopes;
use hir_def::{Body, Expr, ExprId, PatId, Statement};

/// Body validation diagnostics.
pub enum BodyValidationDiagnostic {
    /// Variable binding is never referenced.
    UnusedVariable { pat: PatId, name: String },
    /// Function body ends with an explicit `return` that is redundant.
    RemoveTrailingReturn { return_expr: ExprId },
    /// `else` branch is unnecessary because `then` branch diverges.
    RemoveUnnecessaryElse { if_expr: ExprId },
}

impl BodyValidationDiagnostic {
    /// Collect all body validation diagnostics for a callable body.
    /// Uses `ExprScopes` to enumerate bindings from let/var statements
    /// instead of ad-hoc `let_binding_pats` tracking. `ExprScopes` is built
    /// from the Body and provides the authoritative list of all bindings
    /// introduced in each scope.
    pub fn collect(
        body: &Body,
        expr_scopes: &ExprScopes,
        used_bindings: &HashSet<String>,
        pattern_constants: &HashSet<String>,
    ) -> Vec<Self> {
        let mut result = Vec::new();
        collect_unused_variables(body, expr_scopes, used_bindings, pattern_constants, &mut result);
        collect_trailing_return(body, &mut result);
        collect_unnecessary_else(body, &mut result);
        result
    }
}

/// Detect unused variable bindings via ExprScopes.
///
/// Walks all scopes in ExprScopes. For each binding entry, checks if
/// the name was referenced during inference (via `used_bindings` set).
///
/// Skips:
/// - Root scope entries (function parameters — always "used" by the caller)
/// - Underscore-prefixed names (convention for intentionally unused)
/// - Names that appear in `used_bindings`
///
fn collect_unused_variables(
    body: &Body,
    expr_scopes: &ExprScopes,
    used_bindings: &HashSet<String>,
    pattern_constants: &HashSet<String>,
    out: &mut Vec<BodyValidationDiagnostic>,
) {
    // The root scope (index 0) contains function parameters.
    // Skip those — parameters are not "unused" in the traditional sense.
    let root_scope = expr_scopes.scope_chain(expr_scopes.scope_for(body.root())).last();

    for (scope_id, scope_data) in expr_scopes.all_scopes() {
        // Skip root scope (function parameters)
        if Some(scope_id) == root_scope {
            continue;
        }
        // Skip foreach loop scopes — the iterator binding is never
        // flagged as unused.
        if scope_data.origin() == hir_def::expr_store::scope::ScopeOrigin::ForeachLoop {
            continue;
        }
        for entry in expr_scopes.entries(scope_id) {
            let name = entry.name().as_str();
            // Skip underscore-prefixed
            if name.starts_with('_') {
                continue;
            }
            // Skip type variable bindings (`let 'n = ...`).
            // Used in type annotations (bits('n)) at the type level.
            if name.starts_with('\'') {
                continue;
            }
            // Skip if used
            if used_bindings.contains(name) {
                continue;
            }
            // Skip if it looks like a constructor (capitalized name)
            if name.chars().next().is_some_and(|c| c.is_uppercase()) {
                continue;
            }
            // Skip enum/union/mapping constructor names used as match
            // patterns — these are constants, not variable bindings.
            if pattern_constants.contains(name) {
                continue;
            }
            // Skip non-identifier patterns (expressions like `false`,
            // `write_success & s` that the CST parser treats as pattern
            // bindings but are actually literals or expressions).
            if name.contains(|c: char| !c.is_alphanumeric() && c != '_') {
                continue;
            }
            // Skip keyword literals used as match patterns.
            if matches!(name, "true" | "false" | "bitzero" | "bitone") {
                continue;
            }
            out.push(BodyValidationDiagnostic::UnusedVariable {
                pat: entry.pat(),
                name: name.to_string(),
            });
        }
    }
}

/// Detect trailing `return` that is redundant.
fn collect_trailing_return(body: &Body, out: &mut Vec<BodyValidationDiagnostic>) {
    let root = body.root();
    let Some(root_expr) = body.expr(root) else {
        return;
    };

    if let Expr::Return(_) = root_expr {
        out.push(BodyValidationDiagnostic::RemoveTrailingReturn { return_expr: root });
    }

    if let Expr::Block(stmts) = root_expr {
        if let Some(Statement::Expr(last_id)) = stmts.last() {
            if let Some(Expr::Return(_)) = body.expr(*last_id) {
                out.push(BodyValidationDiagnostic::RemoveTrailingReturn { return_expr: *last_id });
            }
        }
    }
}

/// Detect unnecessary `else` when `then` branch diverges.
fn collect_unnecessary_else(body: &Body, out: &mut Vec<BodyValidationDiagnostic>) {
    for (expr_id, expr) in body.iter_exprs() {
        if let Expr::If { then_branch, else_branch: Some(_), .. } = expr {
            if let Some(then_expr) = body.expr(*then_branch) {
                let diverges =
                    matches!(then_expr, Expr::Return(_) | Expr::Throw(_) | Expr::Exit(_));
                if diverges {
                    out.push(BodyValidationDiagnostic::RemoveUnnecessaryElse { if_expr: expr_id });
                }
            }
        }
    }
}
