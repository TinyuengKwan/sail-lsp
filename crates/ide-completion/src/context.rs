//! Completion context — semantic position analysis.
//! Architecture: `CompletionContext::new()` analyzes the cursor position
//! and produces a `(CompletionContext, CompletionAnalysis)` pair.
//! The analysis enum drives dispatch to specific completion providers.

use ide_db::ide_types::CompletionRelevanceTypeMatch;
use ide_db::FileDb;

//
// Two-phase: context captures semantic state, analysis classifies
// what kind of completion to perform.

/// Classification of the completion position.
///
/// Determines which providers are dispatched.
#[allow(dead_code)]
pub(crate) enum CompletionAnalysis {
    /// Cursor is at a name definition site (e.g., `function |`).
    Name(NameContext),
    /// Cursor is at a name reference site (e.g., expression, type, pattern).
    NameRef(NameRefContext),
    /// Cursor is inside a string literal.
    String,
}

/// Context for name definition completions.
#[allow(dead_code)]
pub(crate) struct NameContext {
    pub(crate) kind: NameKind,
}

/// What kind of name is being defined.
#[allow(dead_code)]
pub(crate) enum NameKind {
    /// Function name definition.
    Function,
    /// Type name definition.
    TypeDef,
    /// Let/var binding name.
    Let,
    /// Function parameter name.
    Param,
    /// Match arm binding.
    MatchArm,
    /// Other name context.
    Other,
}

/// Context for name reference completions (the common case).
#[allow(dead_code)]
pub(crate) struct NameRefContext {
    /// Dot-access: `expr.|`
    pub(crate) dot_access: Option<DotAccess>,
    /// Path context (if inside a path expression).
    pub(crate) path_ctx: Option<PathCompletionCtx>,
    /// Record expression context: `Struct { field: |, ... }`.
    pub(crate) record_expr: bool,
}

/// Dot-access context.
#[allow(dead_code)]
pub(crate) struct DotAccess {
    /// Text of the receiver expression before the dot.
    pub(crate) receiver_text: String,
    /// Resolved type of the receiver expression.
    ///
    /// Sail uses `hir_ty::Ty` directly (no TypeInfo wrapper).
    /// Populated during CompletionContext construction when sema is available.
    pub(crate) receiver_ty: Option<hir_ty::infer::Ty>,
    /// Whether this is field access or method call.
    pub(crate) kind: DotAccessKind,
}

/// Discriminates field vs method access.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub(crate) enum DotAccessKind {
    Field,
    Method,
}

/// Path completion context.
#[allow(dead_code)]
pub(crate) struct PathCompletionCtx {
    /// Whether the call site already has parentheses.
    pub(crate) has_call_parens: bool,
    /// How the path is qualified.
    pub(crate) qualified: Qualified,
    /// What kind of path this is.
    pub(crate) kind: PathKind,
}

/// How the completion trigger is qualified.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum Qualified {
    /// Bare identifier: `foo|`.
    No,
    /// Qualified on a type name: `TypeName.|`.
    With {
        /// The resolved type/module path before the dot.
        path: String,
    },
}

/// What kind of path the completion is in.
///
/// Sail-specific: no `Attr`, `Derive`, `Vis`, `Use` variants.
#[allow(dead_code)]
pub(crate) enum PathKind {
    /// Expression position: value expected.
    Expr { expr_ctx: ExprCtx },
    /// Type annotation position: type expected.
    Type { location: TypeLocation },
    /// Item list position: top-level or block-level item.
    Item { kind: ItemListKind },
    /// Pattern position: pattern expected.
    Pat { pat_ctx: PatternContext },
}

/// Expression completion context.
#[allow(dead_code)]
pub(crate) struct ExprCtx {
    /// Inside a block expression.
    pub(crate) in_block_expr: bool,
    /// Inside a condition (if/while condition).
    pub(crate) in_condition: bool,
    /// Inside a match guard (`if` after pattern).
    pub(crate) in_match_guard: bool,
    /// After an `if` expression (for `else` suggestion).
    pub(crate) after_if_expr: bool,
}

/// Where in a type annotation the completion is.
#[allow(dead_code)]
pub(crate) enum TypeLocation {
    /// After `:` in a let/var binding.
    TypeAscription,
    /// After `->` in a function return type.
    FunctionReturn,
    /// In a function parameter type position.
    FunctionParam,
    /// In a type alias RHS.
    TypeAlias,
    /// Other type position.
    Other,
}

/// Item list context.
#[allow(dead_code)]
pub(crate) enum ItemListKind {
    /// File top-level.
    SourceFile,
    /// Inside a block `{ }`.
    Block,
}

/// Pattern completion context.
#[allow(dead_code)]
pub(crate) struct PatternContext {
    /// Whether this is a refutable pattern (match arm) or irrefutable (let binding).
    pub(crate) refutability: PatternRefutability,
    /// Whether the pattern is inside a record/struct destructuring.
    pub(crate) in_record_pat: bool,
    /// Whether this is a function parameter position (`function foo(|`).
    pub(crate) is_param: bool,
}

/// Whether a pattern context is refutable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum PatternRefutability {
    /// Refutable: match arm, if-let.
    Refutable,
    /// Irrefutable: let binding, function parameter.
    Irrefutable,
}

//
// Used by existing providers. New code should match on
// `CompletionAnalysis` + `PathKind` instead.

/// Completion position kind (backward compat).
///
/// Derived from `CompletionAnalysis` for use in existing providers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionPosition {
    /// After `:` or `->` — only type names expected.
    TypeAnnotation,
    /// Inside a function body / block — expressions + local keywords.
    Expression,
    /// Inside a pattern (match arm, let binding LHS).
    Pattern,
    /// At top level (outside any braces) — top-level keywords + definitions.
    TopLevel,
}

/// Derive `CompletionPosition` from `CompletionAnalysis`.
fn position_from_analysis(analysis: &CompletionAnalysis) -> CompletionPosition {
    match analysis {
        CompletionAnalysis::Name(_) => CompletionPosition::TopLevel,
        CompletionAnalysis::String => CompletionPosition::Expression,
        CompletionAnalysis::NameRef(nr) => {
            if let Some(path) = &nr.path_ctx {
                match &path.kind {
                    PathKind::Type { .. } => CompletionPosition::TypeAnnotation,
                    PathKind::Item { kind: ItemListKind::SourceFile } => {
                        CompletionPosition::TopLevel
                    }
                    PathKind::Pat { .. } => CompletionPosition::Pattern,
                    PathKind::Expr { .. } | PathKind::Item { .. } => CompletionPosition::Expression,
                }
            } else if nr.dot_access.is_some() {
                CompletionPosition::Expression
            } else {
                CompletionPosition::Expression
            }
        }
    }
}

/// Derive `Qualified` from `CompletionAnalysis` for backward compat.
fn qualified_from_analysis(analysis: &CompletionAnalysis) -> Qualified {
    match analysis {
        CompletionAnalysis::NameRef(nr) => {
            if let Some(path) = &nr.path_ctx {
                path.qualified.clone()
            } else if let Some(dot) = &nr.dot_access {
                Qualified::With { path: dot.receiver_text.clone() }
            } else {
                Qualified::No
            }
        }
        _ => Qualified::No,
    }
}

/// Completion context — everything a provider needs to generate items.
pub(crate) struct CompletionContext<'a> {
    /// Semantic analysis access.
    #[allow(dead_code)]
    pub(crate) sema: hir::Semantics<'a>,
    /// Database reference for salsa queries.
    #[allow(dead_code)]
    pub(crate) db: &'a dyn salsa::Database,

    /// Salsa file input (for type queries via sema.type_of_expr).
    /// None when db doesn't have this file registered.
    #[allow(dead_code)]
    pub(crate) file_text: Option<base_db::FileText>,

    /// The expected type of what we are completing.
    ///
    /// E.g., inside `if |` → expected bool, inside `let x: int = |` → expected int.
    #[allow(dead_code)]
    pub(crate) expected_type: Option<hir_ty::infer::Ty>,
    /// The expected name of what we are completing.
    ///
    /// Usually the parameter name of the function argument we are completing.
    #[allow(dead_code)]
    pub(crate) expected_name: Option<String>,

    /// Current file.
    pub(crate) file: &'a dyn FileDb,
    /// Source text.
    pub(crate) text: &'a str,
    /// Byte offset of cursor.
    pub(crate) offset: usize,
    /// Prefix typed so far (identifier fragment before cursor).
    pub(crate) prefix: &'a str,
    /// Detected position (backward compat — derived from analysis).
    pub(crate) position: CompletionPosition,
    /// Whether cursor is at top level (backward compat).
    pub(crate) is_top_level: bool,
    /// Qualification state (backward compat).
    pub(crate) qualified: Qualified,
}

impl<'a> CompletionContext<'a> {
    /// Build context and analysis from db + file + offset.
    ///
    /// ```text
    /// pub(crate) fn new(
    ///     db: &'db RootDatabase,
    ///     position: FilePosition,
    ///     config: &'db CompletionConfig<'db>,
    ///     trigger_character: Option<char>,
    /// ) -> Option<(CompletionContext<'db>, CompletionAnalysis<'db>)>
    /// ```
    pub(crate) fn new(
        db: &'a dyn salsa::Database,
        file: &'a dyn FileDb,
        file_text: Option<base_db::FileText>,
        text: &'a str,
        offset: usize,
        prefix: &'a str,
    ) -> (Self, CompletionAnalysis) {
        let sema = hir::Semantics::new(db);
        let mut analysis = analyze(text, offset, prefix);

        // Resolve receiver type for dot access using sema.
        //   `receiver_ty = receiver.as_ref().and_then(|it| sema.type_of_expr(it))`
        if let CompletionAnalysis::NameRef(ref mut nr) = analysis {
            if let Some(ref mut dot) = nr.dot_access {
                if let Some(ft) = file_text {
                    // Resolve type at the receiver expression offset
                    let receiver_end = offset.saturating_sub(prefix.len()).saturating_sub(1); // before the dot
                    dot.receiver_ty = sema.type_of_expr(ft, receiver_end);
                }
            }
        }

        // Compute expected type and name.
        let (expected_type, expected_name) = expected_type_and_name(text, offset, &analysis);

        let position = position_from_analysis(&analysis);
        let is_top_level = position == CompletionPosition::TopLevel;
        let qualified = qualified_from_analysis(&analysis);
        let ctx = Self {
            sema,
            db,
            file_text,
            expected_type,
            expected_name,
            file,
            text,
            offset,
            prefix,
            position,
            is_top_level,
            qualified,
        };
        (ctx, analysis)
    }

    /// Lowercase prefix for case-insensitive matching.
    pub fn prefix_lower(&self) -> String {
        self.prefix.to_ascii_lowercase()
    }
}

//
// into a CompletionAnalysis variant.

/// Classify the completion position into a `CompletionAnalysis`.
fn analyze(text: &str, offset: usize, prefix: &str) -> CompletionAnalysis {
    let before = &text[..offset];
    let trimmed = before.trim_end();
    let trimmed = trimmed.strip_suffix(prefix).unwrap_or(trimmed).trim_end();

    // Dot access: `expr.|`
    if let Some(dot_access) = detect_dot_access(text, offset, prefix) {
        return CompletionAnalysis::NameRef(NameRefContext {
            dot_access: Some(dot_access),
            path_ctx: None,
            record_expr: false,
        });
    }

    // Type annotation: after `:` or `->`
    if trimmed.ends_with(':') || trimmed.ends_with("->") {
        let location = if trimmed.ends_with("->") {
            TypeLocation::FunctionReturn
        } else {
            TypeLocation::TypeAscription
        };
        return CompletionAnalysis::NameRef(NameRefContext {
            dot_access: None,
            path_ctx: Some(PathCompletionCtx {
                has_call_parens: false,
                qualified: Qualified::No,
                kind: PathKind::Type { location },
            }),
            record_expr: false,
        });
    }

    // Function parameter position: inside `function name(` or `val name : ... (`
    // before any `)`. Detect by scanning backwards for unmatched `(`.
    if is_in_param_list(text, offset) {
        return CompletionAnalysis::NameRef(NameRefContext {
            dot_access: None,
            path_ctx: Some(PathCompletionCtx {
                has_call_parens: false,
                qualified: Qualified::No,
                kind: PathKind::Pat {
                    pat_ctx: PatternContext {
                        refutability: PatternRefutability::Irrefutable,
                        in_record_pat: false,
                        is_param: true,
                    },
                },
            }),
            record_expr: false,
        });
    }

    // Brace depth for top-level detection
    let mut brace_depth = 0i32;
    let bytes = text.as_bytes();
    for i in 0..offset {
        match bytes[i] {
            b'{' => brace_depth += 1,
            b'}' => brace_depth -= 1,
            _ => {}
        }
    }

    // Top-level: item list
    if brace_depth <= 0 {
        return CompletionAnalysis::NameRef(NameRefContext {
            dot_access: None,
            path_ctx: Some(PathCompletionCtx {
                has_call_parens: false,
                qualified: Qualified::No,
                kind: PathKind::Item { kind: ItemListKind::SourceFile },
            }),
            record_expr: false,
        });
    }

    // Pattern context: after match/catch `{` before `=>`
    if let Some(before_brace_idx) = before.rfind('{') {
        let context_text = text[..before_brace_idx].trim_end();
        if context_text.ends_with("catch") || context_text.ends_with("match") {
            let after_brace = text[before_brace_idx + 1..offset].trim_start();
            if !after_brace.contains("=>") {
                return CompletionAnalysis::NameRef(NameRefContext {
                    dot_access: None,
                    path_ctx: Some(PathCompletionCtx {
                        has_call_parens: false,
                        qualified: Qualified::No,
                        kind: PathKind::Pat {
                            pat_ctx: PatternContext {
                                refutability: PatternRefutability::Refutable,
                                in_record_pat: false,
                                is_param: false,
                            },
                        },
                    }),
                    record_expr: false,
                });
            }
        }
    }

    // Expression after match arm `=>`
    let qualified = detect_qualification(text, offset, prefix);

    CompletionAnalysis::NameRef(NameRefContext {
        dot_access: None,
        path_ctx: Some(PathCompletionCtx {
            has_call_parens: false,
            qualified,
            kind: PathKind::Expr {
                expr_ctx: ExprCtx {
                    in_block_expr: brace_depth > 0,
                    in_condition: false,
                    in_match_guard: false,
                    after_if_expr: trimmed.ends_with("else"),
                },
            },
        }),
        record_expr: false,
    })
}

/// Detect dot-access pattern: `expr.|`
fn detect_dot_access(text: &str, offset: usize, prefix: &str) -> Option<DotAccess> {
    let prefix_start = offset.saturating_sub(prefix.len());
    if prefix_start == 0 {
        return None;
    }
    let before = &text[..prefix_start];
    if !before.ends_with('.') {
        return None;
    }
    let dot_pos = prefix_start - 1;
    let bytes = text.as_bytes();
    let mut start = dot_pos;
    while start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
        start -= 1;
    }
    if start == dot_pos {
        return None;
    }
    let receiver_text = text[start..dot_pos].to_string();
    Some(DotAccess { receiver_text, receiver_ty: None, kind: DotAccessKind::Field })
}

/// Detect qualified path: `TypeName.|`
fn detect_qualification(text: &str, offset: usize, prefix: &str) -> Qualified {
    let prefix_start = offset.saturating_sub(prefix.len());
    if prefix_start == 0 {
        return Qualified::No;
    }
    let before = &text[..prefix_start];
    if !before.ends_with('.') {
        return Qualified::No;
    }
    let dot_pos = prefix_start - 1;
    let bytes = text.as_bytes();
    let mut start = dot_pos;
    while start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
        start -= 1;
    }
    if start == dot_pos {
        return Qualified::No;
    }
    let path = &text[start..dot_pos];
    if path.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
        Qualified::With { path: path.to_string() }
    } else {
        Qualified::No
    }
}

// Sail uses text heuristics to infer common expected-type patterns.

/// Infer expected type and name from the completion context.
fn expected_type_and_name(
    text: &str,
    offset: usize,
    analysis: &CompletionAnalysis,
) -> (Option<hir_ty::infer::Ty>, Option<String>) {
    let before = text[..offset].trim_end();

    // If in a type annotation position, no expected *value* type.
    if let CompletionAnalysis::NameRef(nr) = analysis {
        if let Some(path) = &nr.path_ctx {
            if matches!(path.kind, PathKind::Type { .. }) {
                return (None, None);
            }
        }
    }

    // `if | then` → expected bool (condition position)
    if before.ends_with("if") || before.ends_with("else if") {
        return (Some(hir_ty::infer::Ty::scalar(hir_ty::ty::Scalar::Bool)), None);
    }

    // `while | do` → expected bool
    if before.ends_with("while") {
        return (Some(hir_ty::infer::Ty::scalar(hir_ty::ty::Scalar::Bool)), None);
    }

    // `assert(|)` or `constraint(|)` → expected bool
    if before.ends_with("assert(") || before.ends_with("constraint(") {
        return (Some(hir_ty::infer::Ty::scalar(hir_ty::ty::Scalar::Bool)), None);
    }

    // `let x : T = |` or `var x : T = |` → expected T
    // Scan backwards for `= ` preceded by `: TYPE`
    if let Some(eq_pos) = before.rfind('=') {
        let pre_eq = before[..eq_pos].trim_end();
        if let Some(colon_pos) = pre_eq.rfind(':') {
            let type_str = pre_eq[colon_pos + 1..].trim();
            if !type_str.is_empty() {
                let name = extract_binding_name(pre_eq, colon_pos);
                return (Some(hir_ty::infer::Ty::named(type_str)), name);
            }
        }
    }

    // `-> T` return position: detect `{ | }` or `= |` after function header
    // with `-> T`. Scan backwards for `->` to find expected return type.
    if let Some(arrow_pos) = before.rfind("->") {
        let after_arrow = before[arrow_pos + 2..].trim_start();
        // The return type extends until `=` or `{`
        if let Some(end) = after_arrow.find(|c: char| c == '=' || c == '{') {
            let ret_ty = after_arrow[..end].trim();
            if !ret_ty.is_empty()
                && ret_ty.chars().all(|c| {
                    c.is_alphanumeric() || c == '_' || c == '\'' || c == '(' || c == ')' || c == ','
                })
            {
                return (Some(hir_ty::infer::Ty::named(ret_ty)), None);
            }
        }
    }

    // Function argument position: `fname(arg1, |` → try to find expected param type
    // This is a heuristic: find the enclosing call and count comma-separated args.
    if let Some((fn_name, arg_index)) = find_enclosing_call(before) {
        return (None, Some(format!("{fn_name}#{arg_index}")));
    }

    (None, None)
}

/// Extract binding name from `let name : T` or `var name : T`.
fn extract_binding_name(pre_eq: &str, colon_pos: usize) -> Option<String> {
    let before_colon = pre_eq[..colon_pos].trim_end();
    // Last word before the colon is the variable name
    before_colon
        .rsplit(|c: char| c.is_whitespace())
        .next()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// Find the enclosing function call and the argument index at the cursor.
///
/// Given `foo(a, b, |`, returns `Some(("foo", 2))`.
fn find_enclosing_call(before: &str) -> Option<(String, usize)> {
    let mut depth = 0i32;
    let mut commas = 0usize;
    for ch in before.chars().rev() {
        match ch {
            ')' => depth += 1,
            '(' => {
                if depth == 0 {
                    // Found the opening paren — extract function name
                    let paren_offset = before.rfind('(')?;
                    let fn_part = before[..paren_offset].trim_end();
                    let fn_name = fn_part
                        .rsplit(|c: char| !c.is_alphanumeric() && c != '_')
                        .next()
                        .filter(|s| !s.is_empty())?;
                    return Some((fn_name.to_string(), commas));
                }
                depth -= 1;
            }
            ',' if depth == 0 => commas += 1,
            _ => {}
        }
    }
    None
}

/// Compute how well a candidate type matches the expected type.
///
/// Returns `Exact` for identical types, `CouldUnify` for compatible
/// base types (e.g., same ADT name with different args), or `None`
/// if the types are unrelated.
pub(crate) fn compute_type_match(
    expected: &hir_ty::infer::Ty,
    candidate: &hir_ty::infer::Ty,
) -> Option<CompletionRelevanceTypeMatch> {
    if expected.is_error() || candidate.is_error() {
        return None;
    }
    let exp_text = expected.display_text();
    let cand_text = candidate.display_text();
    if exp_text == cand_text {
        Some(CompletionRelevanceTypeMatch::Exact)
    } else if could_types_unify(expected, candidate) {
        Some(CompletionRelevanceTypeMatch::CouldUnify)
    } else {
        None
    }
}

/// Check if two types could potentially unify.
///
/// Returns true if the types share the same base name (e.g., `bits(32)`
/// and `bits(64)` both have base `bits`) or if either is a type parameter.
fn could_types_unify(a: &hir_ty::infer::Ty, b: &hir_ty::infer::Ty) -> bool {
    use hir_ty::ty::TyKind;
    match (a.kind(), b.kind()) {
        // Type parameters can unify with anything.
        (TyKind::Param(_), _) | (_, TyKind::Param(_)) => true,
        // Same scalar types always unify (handled by exact match above,
        // but different scalars do not).
        (TyKind::Scalar(sa), TyKind::Scalar(sb)) => sa == sb,
        // Same ADT base name → could unify (args may differ).
        (TyKind::Adt(na, _), TyKind::Adt(nb, _)) => na == nb,
        // Same App base name → could unify.
        (TyKind::App { name: na, .. }, TyKind::App { name: nb, .. }) => na == nb,
        // ADT vs App with same name (e.g., `bits` as Adt vs App).
        (TyKind::Adt(n, _), TyKind::App { name, .. })
        | (TyKind::App { name, .. }, TyKind::Adt(n, _)) => n == name,
        // Tuples: same arity.
        (TyKind::Tuple(a_items), TyKind::Tuple(b_items)) => a_items.len() == b_items.len(),
        // Function types: same param count.
        (TyKind::FnPtr(fa), TyKind::FnPtr(fb)) => fa.params.len() == fb.params.len(),
        _ => false,
    }
}

/// Check if the cursor is inside a function parameter list.
/// Looks for `function name(` or `mapping name(` before the cursor
/// with an unmatched opening paren.
fn is_in_param_list(text: &str, offset: usize) -> bool {
    let before = &text[..offset];
    // Scan backwards for unmatched `(`.
    let mut depth = 0i32;
    for &b in before.as_bytes().iter().rev() {
        match b {
            b')' => depth += 1,
            b'(' => {
                if depth == 0 {
                    // Found unmatched `(`. Check if preceded by function/mapping name.
                    if let Some(paren_pos) = before.rfind('(') {
                        let line_start = before[..paren_pos].rfind('\n').map(|p| p + 1).unwrap_or(0);
                        let line = before[line_start..paren_pos].trim();
                        return line.starts_with("function ") || line.starts_with("mapping ");
                    }
                    return false;
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    false
}
