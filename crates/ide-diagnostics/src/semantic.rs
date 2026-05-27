//! Semantic diagnostic analysis using HIR Body arenas + ItemTree.
//!
//! multiple layers (parse, name-resolution, type-inference) and adds
//! IDE-specific semantic checks.
//!
//! Each check function walks Body arenas via `iter_exprs()` / `iter_pats()`.

use std::collections::HashMap;

use hir_def::bodies::CallableBodies;
use hir_def::body::ExprId;
use hir_def::diagnostics::{Diagnostic, DiagnosticCode, DiagnosticTag, Severity};
use hir_def::hir::{Expr, Statement};
use hir_def::item_tree::{ItemKind, ItemTree};
use ide_db::FileDb;

/// Compute semantic diagnostics for a single file with workspace context.
pub fn compute_semantic_diagnostics_with_workspace<'a, F, I>(
    file: &dyn FileDb,
    _all_files: I,
) -> Vec<Diagnostic>
where
    F: FileDb + 'a,
    I: IntoIterator<Item = &'a F> + Clone,
{
    // Workspace-aware checks (cross-file unused functions etc.) are
    // deferred — for now delegate to single-file checks.
    compute_semantic_diagnostics(file)
}

/// Compute semantic diagnostics for a single file.
///
/// Only includes checks that walk the AST directly — NOT type inference
/// (which goes through `DefWithBody::diagnostics` → salsa queries).
///
/// Removed checks:
/// - `check_unused_variables`: duplicated by `InferenceDiagnostic::UnusedVariable`
///   in the per-callable inference pipeline.
/// - `check_scattered_completeness_single_file`: single-file scattered check
///   is almost always a false positive.
///   Workspace-level `check_workspace_scattered_completeness()` remains.
pub fn compute_semantic_diagnostics(file: &dyn FileDb) -> Vec<Diagnostic> {
    let text = file.text();
    if text.is_empty() {
        return Vec::new();
    }
    let (cst_root, _) = syntax::parse_text(text);
    let bodies = CallableBodies::from_cst(&cst_root);
    let item_tree = ItemTree::build_from_cst(&cst_root);

    let mut diagnostics = Vec::new();
    check_duplicate_definitions(&item_tree, &mut diagnostics);
    check_duplicate_enum_members(&cst_root, &mut diagnostics);
    check_recursive_types(&cst_root, &mut diagnostics);
    check_unreachable_code(&bodies, &mut diagnostics);
    diagnostics
}

/// Detect duplicate top-level type definitions (structs, enums, etc.).
fn check_duplicate_definitions(item_tree: &ItemTree, diagnostics: &mut Vec<Diagnostic>) {
    let mut seen_types: HashMap<&str, parser::Span> = HashMap::new();
    for &id in item_tree.top_level_items() {
        let is_type = matches!(
            id.item_kind(item_tree),
            ItemKind::Struct
                | ItemKind::Union
                | ItemKind::Enum
                | ItemKind::Bitfield
                | ItemKind::Newtype
                | ItemKind::TypeAlias
        );
        if !is_type {
            continue;
        }
        let name = id.name(item_tree).as_str();
        let span = id.span(item_tree);
        if let Some(&first_span) = seen_types.get(name) {
            diagnostics.push(Diagnostic::new(
                DiagnosticCode::SailError("duplicate-definition"),
                format!("Duplicate definition of `{}`", name),
                base_db::text_range(span.start, span.end),
                Severity::Error,
            ));
            let _ = first_span; // first occurrence not re-diagnosed
        } else {
            seen_types.insert(name, span);
        }
    }
}

/// Detect duplicate enum/union member names within a single definition.
///
/// `check_duplicate_definitions` catches duplicate top-level type names but
/// NOT duplicate variants within one enum/union. This fills that gap.
///
/// Walks CST NAMED_DEF nodes for enums/unions, collects member IDENTs inside
/// braces, and flags duplicates.
fn check_duplicate_enum_members(cst_root: &syntax::SyntaxNode, diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;
    use std::collections::HashSet;

    for node in cst_root.descendants() {
        if node.kind() != SK::NAMED_DEF {
            continue;
        }
        let first_kw = node.children_with_tokens().find_map(|c| c.into_token().map(|t| t.kind()));
        let is_enum = first_kw == Some(SK::KW_ENUM);
        let is_union = first_kw == Some(SK::KW_UNION);
        if !is_enum && !is_union {
            continue;
        }

        let mut seen = HashSet::new();

        if is_enum {
            // Enum: all IDENTs inside braces are member names.
            let mut in_braces = false;
            for tok in node.descendants_with_tokens().filter_map(|el| el.into_token()) {
                match tok.kind() {
                    SK::L_CURLY => in_braces = true,
                    SK::R_CURLY => in_braces = false,
                    SK::IDENT if in_braces => {
                        let name = tok.text().to_string();
                        if !seen.insert(name.clone()) {
                            emit_dup_member(diagnostics, &name, &tok);
                        }
                    }
                    _ => {}
                }
            }
        } else {
            // Union: syntax is `MemberName : Type`, so only the IDENT
            // in FIELD_INIT *before* `:` is the member name.
            for field in node.descendants() {
                if field.kind() != SK::FIELD_INIT {
                    continue;
                }
                // First IDENT token before COLON is the member name.
                for tok in field.descendants_with_tokens().filter_map(|el| el.into_token()) {
                    if tok.kind() == SK::COLON {
                        break;
                    }
                    if tok.kind() == SK::IDENT {
                        let name = tok.text().to_string();
                        if !seen.insert(name.clone()) {
                            emit_dup_member(diagnostics, &name, &tok);
                        }
                        break;
                    }
                }
            }
        }
    }
}

/// Detect recursive type definitions (types that contain themselves).
///
/// A type like `type T = T`, `struct S = { f: S }`, or mutual recursion
/// `struct A = { b: B }` / `struct B = { a: A }` cannot be represented
/// in memory. Uses `hir_ty::representability::is_representable()`.
fn check_recursive_types(cst_root: &syntax::SyntaxNode, diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    // Collect struct/union field types and type alias RHS from the CST.
    // Map: type_name → (field_type_names, span).
    let mut type_fields: HashMap<String, (Vec<String>, parser::Span)> = HashMap::new();

    for node in cst_root.descendants() {
        let kind = node.kind();

        if kind == SK::NAMED_DEF {
            let first_kw =
                node.children_with_tokens().find_map(|c| c.into_token().map(|t| t.kind()));
            let type_name = node
                .children()
                .find(|c| c.kind() == SK::NAME)
                .and_then(|n| n.first_token())
                .map(|t| t.text().to_string());
            let Some(name) = type_name else { continue };

            if matches!(first_kw, Some(SK::KW_STRUCT) | Some(SK::KW_UNION)) {
                // Collect IDENT tokens after ':' in FIELD_INIT children (field types).
                let mut field_types = Vec::new();
                for field in node.descendants() {
                    if field.kind() != SK::FIELD_INIT {
                        continue;
                    }
                    let mut after_colon = false;
                    for tok in field.descendants_with_tokens() {
                        if let Some(t) = tok.as_token() {
                            if t.kind() == SK::COLON {
                                after_colon = true;
                                continue;
                            }
                            if after_colon && t.kind() == SK::IDENT {
                                field_types.push(t.text().to_string());
                                break; // only first IDENT after colon (the type name)
                            }
                        }
                    }
                }
                let range = node.text_range();
                let span = parser::Span {
                    start: u32::from(range.start()) as usize,
                    end: u32::from(range.end()) as usize,
                };
                type_fields.insert(name, (field_types, span));
            }
        } else if kind == SK::TYPE_ALIAS_DEF {
            // `type T = RHS` — only flag direct self-references like `type T = T`.
            // Do NOT flag config expressions, conditional types, or complex RHS.
            let type_name = node
                .children()
                .find(|c| c.kind() == SK::NAME)
                .and_then(|n| n.first_token())
                .map(|t| t.text().to_string());
            let Some(name) = type_name else { continue };

            // Collect all tokens after '=' to check for simple self-reference.
            let mut after_eq = false;
            let mut rhs_tokens = Vec::new();
            for tok in node.descendants_with_tokens().filter_map(|el| el.into_token()) {
                if tok.kind() == SK::EQ {
                    after_eq = true;
                    continue;
                }
                if after_eq && !tok.kind().is_trivia() {
                    rhs_tokens.push((tok.kind(), tok.text().to_string()));
                }
            }
            // Only flag if RHS is exactly one IDENT token matching the name
            // (direct synonym recursion `type T = T`).
            // Complex expressions (config, if/then/else, dot-qualified) are not
            // recursive type errors.
            if rhs_tokens.len() == 1 && rhs_tokens[0].0 == SK::IDENT && rhs_tokens[0].1 == name {
                let range = node.text_range();
                let span = parser::Span {
                    start: u32::from(range.start()) as usize,
                    end: u32::from(range.end()) as usize,
                };
                let self_ref = name.clone();
                type_fields.insert(name, (vec![self_ref], span));
            }
        }
    }

    // Check representability for each type definition.
    let field_lookup = |name: &str| -> Vec<String> {
        type_fields.get(name).map(|(fields, _)| fields.clone()).unwrap_or_default()
    };

    for (name, (_fields, span)) in &type_fields {
        if hir_ty::representability::is_representable(name, &field_lookup)
            == hir_ty::representability::Representability::Infinite
        {
            diagnostics.push(Diagnostic::new(
                DiagnosticCode::SailError("recursive-type"),
                format!("recursive type `{name}` has infinite size"),
                base_db::text_range(span.start, span.end),
                Severity::Error,
            ));
        }
    }
}

fn emit_dup_member(diagnostics: &mut Vec<Diagnostic>, name: &str, tok: &syntax::SyntaxToken) {
    let offset = tok.text_range().start();
    let end = tok.text_range().end();
    diagnostics.push(Diagnostic::new(
        DiagnosticCode::SailError("duplicate-enum-member"),
        format!("duplicate enum/union member `{name}`"),
        base_db::text_range(u32::from(offset) as usize, u32::from(end) as usize),
        Severity::Error,
    ));
}

// check_unused_variables removed — duplicated by
// InferenceDiagnostic::UnusedVariable in per-callable inference pipeline.

/// Detect unreachable code after diverging expressions (return, throw, exit).
///
/// Walks Block statements: if a statement contains a diverging expression,
/// all subsequent statements in the same block are unreachable.
fn check_unreachable_code(bodies: &CallableBodies, diagnostics: &mut Vec<Diagnostic>) {
    for entry in bodies.entries() {
        let body = &entry.body;
        let source_map = &entry.source_map;
        check_unreachable_in_expr(body, source_map, body.root(), diagnostics);
    }
}

fn check_unreachable_in_expr(
    body: &hir_def::Body,
    source_map: &hir_def::BodySourceMap,
    expr_id: ExprId,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(expr) = body.expr(expr_id) else {
        return;
    };
    match expr.clone() {
        Expr::Block(stmts) => {
            let mut diverged = false;
            for stmt in &stmts {
                if diverged {
                    // This statement is unreachable
                    let span = match stmt {
                        Statement::Expr(id) => source_map.expr_syntax(*id),
                        Statement::Let { value, .. } => source_map.expr_syntax(*value),
                        Statement::Var { value, .. } => source_map.expr_syntax(*value), // pat field ignored for span
                    };
                    if let Some(span) = span {
                        diagnostics.push(
                            Diagnostic::new(
                                DiagnosticCode::SailLint("unreachable-code", Severity::Hint),
                                "Unreachable code".to_string(),
                                base_db::text_range(span.start, span.end),
                                Severity::Hint,
                            )
                            .with_tags(vec![DiagnosticTag::Unnecessary]),
                        );
                    }
                    continue;
                }
                // Check if this statement diverges
                match stmt {
                    Statement::Expr(id) => {
                        if expr_diverges(body, *id) {
                            diverged = true;
                        }
                        check_unreachable_in_expr(body, source_map, *id, diagnostics);
                    }
                    Statement::Let { value, .. } => {
                        check_unreachable_in_expr(body, source_map, *value, diagnostics);
                    }
                    Statement::Var { value, .. } => {
                        check_unreachable_in_expr(body, source_map, *value, diagnostics);
                    }
                }
            }
        }
        Expr::If { cond, then_branch, else_branch } => {
            check_unreachable_in_expr(body, source_map, cond, diagnostics);
            check_unreachable_in_expr(body, source_map, then_branch, diagnostics);
            if let Some(else_id) = else_branch {
                check_unreachable_in_expr(body, source_map, else_id, diagnostics);
            }
            // If the then-branch always diverges and there's no else,
            // code after the if in a block is NOT unreachable (control
            // falls through the absent else). This is handled at the
            // Block level above.
        }
        Expr::Match { scrutinee, arms } => {
            check_unreachable_in_expr(body, source_map, scrutinee, diagnostics);
            for arm in &arms {
                check_unreachable_in_expr(body, source_map, arm.body, diagnostics);
            }
        }
        Expr::Let { value, body: let_body, .. } => {
            check_unreachable_in_expr(body, source_map, value, diagnostics);
            check_unreachable_in_expr(body, source_map, let_body, diagnostics);
        }
        _ => {} // other expressions don't introduce blocks
    }
}

/// Check if an expression always diverges (return, throw, exit).
fn expr_diverges(body: &hir_def::Body, expr_id: ExprId) -> bool {
    let Some(expr) = body.expr(expr_id) else {
        return false;
    };
    match expr {
        Expr::Return(_) | Expr::Throw(_) | Expr::Exit(_) => true,
        Expr::If { then_branch, else_branch: Some(else_branch), .. } => {
            // Both branches must diverge for the if to diverge
            expr_diverges(body, *then_branch) && expr_diverges(body, *else_branch)
        }
        Expr::Block(stmts) => {
            // A block diverges if any statement diverges
            stmts.iter().any(|stmt| match stmt {
                Statement::Expr(id) => expr_diverges(body, *id),
                _ => false,
            })
        }
        _ => false,
    }
}

// check_scattered_completeness_single_file removed — single-file
// scattered check is almost always false positive.
// Workspace-level check_workspace_scattered_completeness() remains.

/// Workspace-level scattered completeness check.
///
/// Aggregates scattered status across ALL files, then checks completeness.
/// Call this after workspace scan to detect cross-file scattered problems.
pub fn check_workspace_scattered_completeness(item_trees: &[&ItemTree]) -> Vec<Diagnostic> {
    use hir_def::scattered::{check_scattered_completeness, workspace_scattered_status};

    let status = workspace_scattered_status(item_trees.iter().copied());
    let scattered_diags = check_scattered_completeness(&status);

    scattered_diags.iter().map(scattered_diag_to_hir_diag).collect()
}

/// Check include graph for cycles and emit diagnostics.
///
/// Takes the IncludeGraph built during workspace scan and a URL lookup
/// function for producing human-readable cycle descriptions.
///
/// Returns diagnostics for each file participating in a cycle.
pub fn check_circular_includes(
    include_graph: &hir_def::include_graph::IncludeGraph,
    file_name: &dyn Fn(base_db::FileId) -> String,
) -> Vec<Diagnostic> {
    let Some(cycle_files) = include_graph.find_cycle() else {
        return Vec::new();
    };

    // Build a human-readable cycle description.
    let cycle_names: Vec<String> = cycle_files.iter().map(|&fid| file_name(fid)).collect();
    let cycle_desc = if cycle_names.len() <= 5 {
        cycle_names.join(" → ")
    } else {
        format!("{} → ... ({} files in cycle)", cycle_names[0], cycle_names.len())
    };

    // Emit one diagnostic per file in the cycle (at file start).
    cycle_files
        .iter()
        .map(|_fid| {
            Diagnostic::new(
                DiagnosticCode::SailError("circular-include"),
                format!("circular $include detected: {cycle_desc}"),
                base_db::text_range(0, 0),
                Severity::Error,
            )
        })
        .collect()
}

/// Convert a `ScatteredDiagnostic` to a `hir_def::diagnostics::Diagnostic`.
fn scattered_diag_to_hir_diag(d: &hir_def::scattered::ScatteredDiagnostic) -> Diagnostic {
    use hir_def::scattered::ScatteredDiagnosticKind;

    let message = match &d.kind {
        ScatteredDiagnosticKind::MissingEnd => {
            format!("scattered definition `{}` is missing `end {}`", d.name, d.name)
        }
        ScatteredDiagnosticKind::NoClauses => {
            format!("scattered definition `{}` has no clauses", d.name)
        }
        ScatteredDiagnosticKind::OrphanEnd => {
            format!("`end {}` without a corresponding `scattered` declaration", d.name)
        }
    };
    Diagnostic::new(
        DiagnosticCode::SailLint("incomplete-scattered", Severity::Warning),
        message,
        base_db::text_range(d.span.start, d.span.end),
        Severity::Warning,
    )
}
