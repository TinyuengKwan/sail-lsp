//! Completion context — semantic position analysis.
//!
//! `analyze` used to answer `CompletionAnalysis::NameRef` at every
//! position, so `Name` and `String` — the other two arms, RA's model
//! carried over whole — were unreachable, and thirteen
//! `#[expect(dead_code)]` kept the build quiet about the model underneath
//! them. It distinguishes all three now: inside a string literal nothing
//! completes, and where a name is being *defined* the only candidates are
//! declarations still waiting for it (`completions/name.rs`).
//!
//! Five `#[expect(dead_code)]` are left, on `PathKind::Type`, `ExprCtx`,
//! `TypeLocation`, `ItemListKind` and `PatternContext`: the analyzer fills them in and no
//! provider asks. Each names a distinction Sail's completion could draw —
//! `then`/`else` belong in a condition, `end` in a block — and drawing it
//! is provider work, not analyzer work. What is deleted rather than left
//! is what carried no information at all: `record_expr` and
//! `has_call_parens` were assigned `false` at every construction,
//! `DotAccessKind` was `Field` at the only one, and `NameKind` had three
//! variants nothing produced.
//! Architecture: `CompletionContext::new()` analyzes the cursor position
//! and produces a `(CompletionContext, CompletionAnalysis)` pair.
//! The analysis enum drives dispatch to specific completion providers.

use crate::item::CompletionRelevanceTypeMatch;
use ide_db::FileDb;

//
// Two-phase: context captures semantic state, analysis classifies
// what kind of completion to perform.

/// Classification of the completion position.
///
/// Determines which providers are dispatched.
pub(crate) enum CompletionAnalysis {
    /// Cursor is at a name definition site (e.g., `function |`).
    Name(NameContext),
    /// Cursor is at a name reference site (e.g., expression, type, pattern).
    NameRef(NameRefContext),
    /// Cursor is inside a string literal.
    String,
    /// Cursor is inside a comment. Nothing in the workspace is a
    /// candidate there, and this had no arm at all: typing in `// fo|`
    /// offered every symbol.
    Comment,
}

/// Context for name definition completions.
pub(crate) struct NameContext {
    pub(crate) kind: NameKind,
}

/// What kind of name is being defined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NameKind {
    /// Function name definition.
    Function,
    /// Type name definition.
    TypeDef,
    /// Let/var/register binding name.
    Let,
}

/// Context for name reference completions (the common case).
pub(crate) struct NameRefContext {
    /// Dot-access: `expr.|`
    pub(crate) dot_access: Option<DotAccess>,
    /// Path context (if inside a path expression).
    pub(crate) path_ctx: Option<PathCompletionCtx>,
}

/// Dot-access context.
pub(crate) struct DotAccess {
    /// Text of the receiver expression before the dot.
    pub(crate) receiver_text: String,
    /// Resolved type of the receiver expression.
    ///
    /// Sail uses `hir_ty::Ty` directly (no TypeInfo wrapper).
    /// Populated during CompletionContext construction when sema is available.
    pub(crate) receiver_ty: Option<hir::Ty>,
}

/// Path completion context.
pub(crate) struct PathCompletionCtx {
    /// How the path is qualified.
    pub(crate) qualified: Qualified,
    /// What kind of path this is.
    pub(crate) kind: PathKind,
}

/// How the completion trigger is qualified.
#[derive(Clone, Debug, PartialEq, Eq)]
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
pub(crate) enum PathKind {
    /// Expression position: value expected.
    Expr { expr_ctx: ExprCtx },
    /// Type annotation position: type expected.
    #[expect(dead_code)]
    Type { location: TypeLocation },
    /// Item list position: top-level or block-level item.
    Item { kind: ItemListKind },
    /// Pattern position: pattern expected.
    Pat { pat_ctx: PatternContext },
}

/// Expression completion context.
pub(crate) struct ExprCtx {
    /// Inside a block expression.
    #[expect(dead_code)]
    pub(crate) in_block_expr: bool,
    /// Inside a condition (if/while condition).
    #[expect(dead_code)]
    pub(crate) in_condition: bool,
    /// Inside a match guard (`if` after pattern).
    #[expect(dead_code)]
    pub(crate) in_match_guard: bool,
    /// After an `if` expression (for `else` suggestion).
    #[expect(dead_code)]
    pub(crate) after_if_expr: bool,
}

/// Where in a type annotation the completion is.
pub(crate) enum TypeLocation {
    /// After `:` in a let/var binding.
    TypeAscription,
    /// After `->` in a function return type.
    FunctionReturn,
    /// In a function parameter type position.
    #[expect(dead_code)]
    FunctionParam,
    /// In a type alias RHS.
    #[expect(dead_code)]
    TypeAlias,
    /// Other type position.
    #[expect(dead_code)]
    Other,
}

/// Item list context.
pub(crate) enum ItemListKind {
    /// File top-level.
    SourceFile,
    /// Inside a block `{ }`.
    #[expect(dead_code)]
    Block,
}

/// Pattern completion context.
pub(crate) struct PatternContext {
    /// Whether this is a refutable pattern (match arm) or irrefutable (let binding).
    #[expect(dead_code)]
    pub(crate) refutability: PatternRefutability,
    /// Whether the pattern is inside a record/struct destructuring.
    #[expect(dead_code)]
    pub(crate) in_record_pat: bool,
    /// Whether this is a function parameter position (`function foo(|`).
    pub(crate) is_param: bool,
}

/// Whether a pattern context is refutable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
        CompletionAnalysis::String | CompletionAnalysis::Comment => CompletionPosition::Expression,
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
    pub(crate) sema: hir::Semantics<'a>,
    /// Database reference for salsa queries.
    pub(crate) db: &'a dyn salsa::Database,

    /// Salsa file input (for type queries via sema.type_of_expr).
    /// None when db doesn't have this file registered.
    pub(crate) file_text: Option<base_db::FileText>,

    /// The expected type of what we are completing.
    ///
    /// E.g., inside `if |` → expected bool, inside `let x: int = |` → expected int.
    pub(crate) expected_type: Option<hir::Ty>,
    /// The expected name of what we are completing.
    ///
    /// Usually the parameter name of the function argument we are completing.
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
        // Ask the tree what the cursor is inside before reading the text.
        // `is_in_comment` / `is_in_string` (`ide_db::syntax_helpers`) are
        // one `matches!` on the token kind and had no caller, while this
        // module scanned the raw characters for an unterminated quote —
        // beside a lossless CST that has a `STRING_LIT` token covering
        // exactly that text. And nothing asked about comments at all.
        let mut analysis = match token_before(file, offset) {
            Some(t) if ide_db::syntax_helpers::node_ext::is_in_comment(&t) => {
                CompletionAnalysis::Comment
            }
            Some(t) if ide_db::syntax_helpers::node_ext::is_in_string(&t) => {
                CompletionAnalysis::String
            }
            token => analyze(token.as_ref(), text, offset, prefix),
        };

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
/// The token the cursor sits in, taken from the tree.
///
/// Left-biased at the offset, because a cursor at the end of `// foo` is
/// between that token and whatever follows, and what the user is typing
/// into is the one on the left.
fn token_before(file: &dyn FileDb, offset: usize) -> Option<syntax::SyntaxToken> {
    let root = file.syntax()?;
    let at =
        syntax::TextSize::new(u32::try_from(offset).ok()?.min(u32::from(root.text_range().end())));
    root.token_at_offset(at).left_biased()
}

/// Whether `before` ends inside an unterminated string literal.
fn in_string_literal(before: &str) -> bool {
    let mut in_string = false;
    let mut escaped = false;
    for ch in before.chars() {
        match ch {
            _ if escaped => escaped = false,
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            '\n' => in_string = false,
            _ => {}
        }
    }
    in_string
}

/// The kind of definition whose name is being typed, if that is what this
/// position is.
fn name_being_defined(trimmed: &str) -> Option<NameKind> {
    let last = trimmed.rsplit(|c: char| c.is_whitespace()).find(|w| !w.is_empty())?;
    Some(match last {
        "function" | "mapping" | "val" | "outcome" => NameKind::Function,
        "struct" | "union" | "enum" | "type" | "newtype" | "bitfield" => NameKind::TypeDef,
        "let" | "var" | "register" => NameKind::Let,
        _ => return None,
    })
}

fn analyze(
    token: Option<&syntax::SyntaxToken>,
    text: &str,
    offset: usize,
    prefix: &str,
) -> CompletionAnalysis {
    let before = &text[..offset];
    let trimmed = before.trim_end();
    let trimmed = trimmed.strip_suffix(prefix).unwrap_or(trimmed).trim_end();

    // Inside a string literal, where nothing in the workspace is a
    // candidate. This arm and the one below were never constructed —
    // `analyze` always answered `NameRef` — so typing inside a string, or
    // typing the *name* of a definition, offered every symbol the
    // workspace has.
    if in_string_literal(before) {
        return CompletionAnalysis::String;
    }

    // The name being given to a definition: `function fo|`, `struct S|`.
    // A new name is not a reference to an existing one.
    if let Some(kind) = name_being_defined(trimmed) {
        return CompletionAnalysis::Name(NameContext { kind });
    }

    if let Some(node) = token.and_then(|t| {
        t.parent_ancestors().find(|n| n.kind() == syntax::SyntaxKind::INSTANTIATION_DEF)
    }) {
        let role = syntax::instantiation_reference(&node, offset.saturating_sub(1));
        let kind = match role {
            Some(syntax::InstantiationReference::Type) => {
                PathKind::Type { location: TypeLocation::TypeAscription }
            }
            Some(syntax::InstantiationReference::Callable) => PathKind::Expr {
                expr_ctx: ExprCtx {
                    in_block_expr: false,
                    in_condition: false,
                    in_match_guard: false,
                    after_if_expr: false,
                },
            },
            Some(syntax::InstantiationReference::Outcome) => PathKind::Expr {
                expr_ctx: ExprCtx {
                    in_block_expr: false,
                    in_condition: false,
                    in_match_guard: false,
                    after_if_expr: false,
                },
            },
            None => return CompletionAnalysis::Name(NameContext { kind: NameKind::Let }),
        };
        return CompletionAnalysis::NameRef(NameRefContext {
            dot_access: None,
            path_ctx: Some(PathCompletionCtx { qualified: Qualified::No, kind }),
        });
    }

    // Dot access: `expr.|`
    if let Some(dot_access) = detect_dot_access(text, offset, prefix) {
        return CompletionAnalysis::NameRef(NameRefContext {
            dot_access: Some(dot_access),
            path_ctx: None,
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
                qualified: Qualified::No,
                kind: PathKind::Type { location },
            }),
        });
    }

    // Function parameter position: inside `function name(` or `val name : ... (`
    // before any `)`. Detect by scanning backwards for unmatched `(`.
    // An unmatched call parenthesis on a `function` line is still an
    // expression. Prefer the CST before the incomplete-parameter fallback.
    let in_expression = token.is_some_and(|t| {
        t.parent_ancestors().any(|n| ide_db::syntax_helpers::node_ext::is_expr_kind(n.kind()))
    });
    if !in_expression && is_in_param_list(text, offset) {
        return CompletionAnalysis::NameRef(NameRefContext {
            dot_access: None,
            path_ctx: Some(PathCompletionCtx {
                qualified: Qualified::No,
                kind: PathKind::Pat {
                    pat_ctx: PatternContext {
                        refutability: PatternRefutability::Irrefutable,
                        in_record_pat: false,
                        is_param: true,
                    },
                },
            }),
        });
    }

    // In an expression, which the tree knows and brace counting does not.
    //
    // `function f() -> bits(8) = a_dec|` is at brace depth 0, so the check
    // below read it as the top level of the file and offered item keywords
    // — none of which start with what was typed, so completion answered
    // *nothing* at the one position where a name is most obviously wanted.
    // The same text inside a block answered, which is how it stayed hidden.
    // The token's ancestors say it plainly: `IDENT@115..120` under
    // `IDENT_EXPR@115..120`.
    if in_expression {
        return CompletionAnalysis::NameRef(NameRefContext {
            dot_access: None,
            path_ctx: Some(PathCompletionCtx {
                qualified: detect_qualification(text, offset, prefix),
                kind: PathKind::Expr {
                    expr_ctx: ExprCtx {
                        in_block_expr: token.is_some_and(|t| {
                            t.parent_ancestors().any(|n| n.kind() == syntax::SyntaxKind::BLOCK_EXPR)
                        }),
                        in_condition: false,
                        in_match_guard: false,
                        after_if_expr: trimmed.ends_with("else"),
                    },
                },
            }),
        });
    }

    // Brace depth for top-level detection
    let mut brace_depth = 0i32;
    let bytes = text.as_bytes();
    for &b in bytes.iter().take(offset) {
        match b {
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
                qualified: Qualified::No,
                kind: PathKind::Item { kind: ItemListKind::SourceFile },
            }),
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
                        qualified: Qualified::No,
                        kind: PathKind::Pat {
                            pat_ctx: PatternContext {
                                refutability: PatternRefutability::Refutable,
                                in_record_pat: false,
                                is_param: false,
                            },
                        },
                    }),
                });
            }
        }
    }

    // Expression after match arm `=>`
    let qualified = detect_qualification(text, offset, prefix);

    CompletionAnalysis::NameRef(NameRefContext {
        dot_access: None,
        path_ctx: Some(PathCompletionCtx {
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
    Some(DotAccess { receiver_text, receiver_ty: None })
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
) -> (Option<hir::Ty>, Option<String>) {
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
        return (Some(hir::Ty::scalar(hir::Scalar::Bool)), None);
    }

    // `while | do` → expected bool
    if before.ends_with("while") {
        return (Some(hir::Ty::scalar(hir::Scalar::Bool)), None);
    }

    // `assert(|)` or `constraint(|)` → expected bool
    if before.ends_with("assert(") || before.ends_with("constraint(") {
        return (Some(hir::Ty::scalar(hir::Scalar::Bool)), None);
    }

    // `let x : T = |` or `var x : T = |` → expected T
    // Scan backwards for `= ` preceded by `: TYPE`
    if let Some(eq_pos) = before.rfind('=') {
        let pre_eq = before[..eq_pos].trim_end();
        if let Some(colon_pos) = pre_eq.rfind(':') {
            let type_str = pre_eq[colon_pos + 1..].trim();
            if !type_str.is_empty() {
                let name = extract_binding_name(pre_eq, colon_pos);
                return (Some(hir::Ty::named(type_str)), name);
            }
        }
    }

    // `-> T` return position: detect `{ | }` or `= |` after function header
    // with `-> T`. Scan backwards for `->` to find expected return type.
    if let Some(arrow_pos) = before.rfind("->") {
        let after_arrow = before[arrow_pos + 2..].trim_start();
        // The return type extends until `=` or `{`
        if let Some(end) = after_arrow.find(['=', '{']) {
            let ret_ty = after_arrow[..end].trim();
            if !ret_ty.is_empty()
                && ret_ty.chars().all(|c| {
                    c.is_alphanumeric() || c == '_' || c == '\'' || c == '(' || c == ')' || c == ','
                })
            {
                return (Some(hir::Ty::named(ret_ty)), None);
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
    expected: &hir::Ty,
    candidate: &hir::Ty,
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
fn could_types_unify(a: &hir::Ty, b: &hir::Ty) -> bool {
    use hir::TyKind;
    match (a.kind(), b.kind()) {
        // Type parameters can unify with anything.
        (TyKind::Param(_), _) | (_, TyKind::Param(_)) => true,
        // Same scalar types always unify (handled by exact match above,
        // but different scalars do not).
        (TyKind::Scalar(sa), TyKind::Scalar(sb)) => sa == sb,
        // Same base name → could unify (args may differ).
        (TyKind::Named(na, _), TyKind::Named(nb, _)) => na == nb,
        // Same App base name → could unify.
        (TyKind::App { name: na, .. }, TyKind::App { name: nb, .. }) => na == nb,
        // Named vs App with the same name: the same type written two ways.
        (TyKind::Named(n, _), TyKind::App { name, .. })
        | (TyKind::App { name, .. }, TyKind::Named(n, _)) => n == name,
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
                        let line_start =
                            before[..paren_pos].rfind('\n').map(|p| p + 1).unwrap_or(0);
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

/// The identifier characters a completion prefix is made of. Sail's
/// include `?`, `'` and `~`, which Rust's do not.
fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'?' | b'\'' | b'~')
}

/// The word being typed at `offset`, which is what the completion list is
/// filtered by. It lived in the crate root beside `completions`;
/// rust-analyzer keeps the equivalent on `CompletionContext`, which is
/// where everything else that reads the cursor's surroundings lives.
pub fn completion_prefix(text: &str, offset: usize) -> &str {
    let offset = offset.min(text.len());
    let bytes = text.as_bytes();
    let mut start = offset;

    while start > 0 && is_identifier_byte(bytes[start - 1]) {
        start -= 1;
    }

    &text[start..offset]
}

impl CompletionContext<'_> {
    pub(crate) fn instantiation_role(&self) -> Option<syntax::InstantiationReference> {
        let token = token_before(self.file, self.offset)?;
        let node =
            token.parent_ancestors().find(|n| n.kind() == syntax::SyntaxKind::INSTANTIATION_DEF)?;
        syntax::instantiation_reference(&node, self.offset.saturating_sub(1))
    }

    pub(crate) fn can_see_member(&self, target: &dyn FileDb, member: &hir::FieldDecl) -> bool {
        if self.instantiation_role().is_some() {
            return false;
        }
        let (Some(from), Some((_, target))) = (self.file_text, target.file_text()) else {
            return true;
        };
        use hir::source_order::{
            self,
            bindings::{self, Namespace},
        };
        if source_order::declarations_for(self.db, from).is_some() {
            let Some(at) = source_order::at_offset(self.db, from, self.offset)
                .or_else(|| source_order::at_offset(self.db, from, self.offset.saturating_sub(1)))
            else {
                return false;
            };
            if !matches!(
                source_order::program(self.db, from),
                Some(source_order::Program::Inline(_))
            ) || bindings::contains_name(self.db, at, &member.name)
            {
                let selected =
                    bindings::resolve(self.db, at, member.name.clone(), Namespace::Value);
                return selected.is_some()
                    && selected
                        == source_order::at_offset(self.db, target, member.name_span.start);
            }
        }
        hir::visibility::symbol_visible_from(
            self.db,
            from.file_id(self.db),
            target,
            member.name.as_str(),
        )
    }

    pub(crate) fn can_see(&self, target: &dyn FileDb, name: &str) -> bool {
        match (self.file_text, target.file_text()) {
            (Some(from), Some((_, target))) => {
                if hir::source_order::declarations_for(self.db, from).is_some() {
                    use hir::source_order::{
                        self,
                        bindings::{self, Namespace},
                    };
                    let Some(at) =
                        source_order::at_offset(self.db, from, self.offset).or_else(|| {
                            source_order::at_offset(self.db, from, self.offset.saturating_sub(1))
                        })
                    else {
                        return false;
                    };
                    if matches!(
                        source_order::program(self.db, from),
                        Some(source_order::Program::Inline(_))
                    ) && !bindings::contains_name(self.db, at, &hir::Name::new(name))
                    {
                        return hir::visibility::symbol_visible_from(
                            self.db,
                            from.file_id(self.db),
                            target,
                            name,
                        );
                    }
                    let namespaces = match self.instantiation_role() {
                        Some(syntax::InstantiationReference::Outcome) => vec![Namespace::Outcome],
                        Some(syntax::InstantiationReference::Type) => vec![Namespace::Type],
                        Some(syntax::InstantiationReference::Callable) => vec![Namespace::Callable],
                        None => vec![Namespace::Value, Namespace::Type],
                    };
                    return namespaces.into_iter().any(|ns| {
                        bindings::resolve(self.db, at, hir::Name::new(name), ns)
                            .is_some_and(|d| d.file == target)
                    });
                }
                hir::visibility::symbol_visible_from(self.db, from.file_id(self.db), target, name)
            }
            _ => true,
        }
    }

    pub(crate) fn can_see_item(
        &self,
        target: &dyn FileDb,
        tree: &hir::ItemTree,
        item: hir::DefItem,
    ) -> bool {
        if matches!(
            self.instantiation_role(),
            Some(
                syntax::InstantiationReference::Callable | syntax::InstantiationReference::Outcome
            )
        ) && !matches!(
            item.def_kind(tree),
            hir::DefKind::Function
                | hir::DefKind::ValSpec
                | hir::DefKind::Mapping
                | hir::DefKind::MappingSpec
        ) {
            return false;
        }
        if !self.can_see(target, item.name(tree).as_str()) {
            return false;
        }
        let (Some(from), Some((_, target))) = (self.file_text, target.file_text()) else {
            return true;
        };
        if hir::source_order::declarations_for(self.db, from).is_none() {
            return true;
        }
        use hir::source_order::{
            self,
            bindings::{self, Namespace},
        };
        let Some(at) = source_order::at_offset(self.db, from, self.offset)
            .or_else(|| source_order::at_offset(self.db, from, self.offset.saturating_sub(1)))
        else {
            return false;
        };
        if matches!(source_order::program(self.db, from), Some(source_order::Program::Inline(_)))
            && !bindings::contains_name(self.db, at, item.name(tree))
        {
            return true;
        }

        let Some(candidate) = source_order::at_offset(self.db, target, item.name_span(tree).start)
        else {
            return false;
        };
        let ns = match item.def_kind(tree) {
            hir::DefKind::ScatteredHead if item.scattered_type_kind(tree).is_some() => {
                Namespace::Type
            }
            hir::DefKind::Struct
            | hir::DefKind::Union
            | hir::DefKind::Enum
            | hir::DefKind::Bitfield
            | hir::DefKind::Newtype
            | hir::DefKind::TypeAlias => Namespace::Type,
            _ => Namespace::Value,
        };
        let ns = match self.instantiation_role() {
            Some(syntax::InstantiationReference::Outcome) => Namespace::Outcome,
            Some(syntax::InstantiationReference::Callable) => Namespace::Callable,
            Some(syntax::InstantiationReference::Type) => Namespace::Type,
            None => ns,
        };
        bindings::resolve(self.db, at, item.name(tree).clone(), ns) == Some(candidate)
    }
}
