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
//! Three `#[expect(dead_code)]` are left, on `ExprCtx`, `ItemListKind` and
//! `PatternContext`: the analyzer fills them in and no
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
    /// Type position: a type expected, wherever the tree puts one.
    Type,
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
    /// The type whose field this position names, when it is a field slot:
    /// the index of `v[F]` or a name before `=` in `[o with F = ..]`.
    pub(crate) field_of: Option<hir::Ty>,
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
                    PathKind::Type => CompletionPosition::TypeAnnotation,
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
    pub(crate) config: crate::CompletionConfig,
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
    pub(crate) is_callee: bool,

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
    /// What the cursor names inside an `instantiation`, if it is in one.
    /// Fixed for the request; visibility asks it once per candidate.
    instantiation_role: Option<syntax::InstantiationReference>,
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
        let original_token = token_before(file, offset);
        let in_literal = original_token.as_ref().is_some_and(|token| {
            ide_db::syntax_helpers::node_ext::is_in_comment(token)
                || ide_db::syntax_helpers::node_ext::is_in_string(token)
        });
        // Parse a temporary identifier at the cursor, as RA does. The real
        // source and semantic database stay unchanged. Nodes before the marker
        // keep their original offsets, so callee/receiver queries remain valid.
        let token = if in_literal {
            original_token
        } else {
            speculative_token(db, file_text, offset).or(original_token)
        };
        let mut analysis = match token.as_ref() {
            Some(t) if ide_db::syntax_helpers::node_ext::is_in_comment(t) => {
                CompletionAnalysis::Comment
            }
            Some(t) if ide_db::syntax_helpers::node_ext::is_in_string(t) => {
                CompletionAnalysis::String
            }
            token => analyze(token, text, offset, prefix),
        };

        // Resolve receiver type for dot access using sema.
        //   `receiver_ty = receiver.as_ref().and_then(|it| sema.type_of_expr(it))`
        if let CompletionAnalysis::NameRef(ref mut nr) = analysis {
            if let Some(ref mut dot) = nr.dot_access {
                if let Some(ft) = file_text {
                    // The receiver's last byte, the one before the dot: the
                    // dot's own offset answered with the type of whatever
                    // follows it — an inference variable, so this path
                    // never matched and every field came from the text
                    // fallback.
                    let dot_at = offset.saturating_sub(prefix.len()).saturating_sub(1);
                    dot.receiver_ty = sema.type_of_expr(ft, dot_at.saturating_sub(1));
                }
            }
        }

        // A field slot asks for the type of the expression it belongs to.
        if let CompletionAnalysis::NameRef(NameRefContext {
            path_ctx: Some(PathCompletionCtx { kind: PathKind::Expr { ref mut expr_ctx }, .. }),
            ..
        }) = analysis
        {
            expr_ctx.field_of = token
                .as_ref()
                .and_then(field_slot_base)
                .zip(file_text)
                // An offset inside the base: its end is the next token's.
                .and_then(|(base, ft)| {
                    sema.type_of_expr(ft, usize::from(base.text_range().end()) - 1)
                });
        }

        // Compute expected type and name.
        let (expected_type, expected_name) = match &analysis {
            CompletionAnalysis::NameRef(NameRefContext {
                path_ctx: Some(PathCompletionCtx { kind: PathKind::Expr { .. }, .. }),
                ..
            }) => expected_type_and_name(&sema, db, file_text, token.as_ref()),
            _ => (None, None),
        };

        let instantiation_role = token.as_ref().and_then(|token| {
            let node = token
                .parent_ancestors()
                .find(|n| n.kind() == syntax::SyntaxKind::INSTANTIATION_DEF)?;
            syntax::instantiation_reference(&node, offset.saturating_sub(1))
        });
        let position = position_from_analysis(&analysis);
        let is_top_level = position == CompletionPosition::TopLevel;
        let qualified = qualified_from_analysis(&analysis);
        let is_callee = token.as_ref().and_then(|t| t.parent()).is_some_and(|name| {
            name.kind() == syntax::SyntaxKind::IDENT_EXPR
                && name.parent().is_some_and(|call| {
                    call.kind() == syntax::SyntaxKind::CALL_EXPR
                        && call.first_child().as_ref() == Some(&name)
                })
        });
        let ctx = Self {
            is_callee,
            sema,
            db,
            file_text,
            config: crate::CompletionConfig::default(),
            expected_type,
            expected_name,
            file,
            text,
            offset,
            prefix,
            position,
            is_top_level,
            qualified,
            instantiation_role,
        };
        (ctx, analysis)
    }

    /// Anchor semantic queries in original syntax when the speculative name
    /// lies past an unfinished node's end, possibly after trailing whitespace.
    pub(crate) fn semantic_offset(&self) -> usize {
        let start = self.offset.saturating_sub(self.prefix.len());
        if self.prefix.is_empty() {
            self.text[..start].trim_end().len().saturating_sub(1)
        } else {
            start
        }
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

fn speculative_token(
    db: &dyn salsa::Database,
    file: Option<base_db::FileText>,
    offset: usize,
) -> Option<syntax::SyntaxToken> {
    const MARKER: &str = "sailCompletionMarker";
    let parsed = syntax::parse_query::parse_file(db, file?);
    let at = syntax::TextSize::new(u32::try_from(offset).ok()?);
    let root = parsed.reparse(syntax::TextRange::empty(at), MARKER).syntax_node()?;
    root.token_at_offset(at + syntax::TextSize::new(1)).right_biased()
}

/// Whether `token` is a name inside a type: its nearest enclosing node is
/// one of the type forms, not an expression, pattern or effect set.
fn in_type(token: &syntax::SyntaxToken) -> bool {
    use syntax::SyntaxKind as SK;
    token.kind() == SK::IDENT
        && token.parent().is_some_and(|node| {
            matches!(node.kind(), SK::TYPE_NAMED | SK::TYPE_APP)
                && node.ancestors().all(|n| n.kind() != SK::TYPE_EFFECT)
        })
}

/// The expression whose field the name at `token` is, if it is in a field
/// slot: the index of `v[F]`, or a name directly before `=` in
/// `[o with F = ..]`. Both parse as a plain `IDENT_EXPR`; only the position
/// says it names a field.
fn field_slot_base(token: &syntax::SyntaxToken) -> Option<syntax::SyntaxNode> {
    use syntax::SyntaxKind as SK;
    let name = token.parent().filter(|n| n.kind() == SK::IDENT_EXPR)?;
    let parent = name.parent()?;
    let base = parent.first_child()?;
    if base == name {
        return None;
    }
    match parent.kind() {
        SK::INDEX_EXPR => Some(base),
        SK::VECTOR_UPDATE_EXPR => {
            let next = name.next_sibling_or_token();
            let next = std::iter::successors(next, |e| e.next_sibling_or_token())
                .find(|e| !e.kind().is_trivia())?;
            (next.kind() == SK::EQ).then_some(base)
        }
        _ => None,
    }
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
            Some(syntax::InstantiationReference::Type) => PathKind::Type,
            Some(syntax::InstantiationReference::Callable) => PathKind::Expr {
                expr_ctx: ExprCtx {
                    in_block_expr: false,
                    in_condition: false,
                    in_match_guard: false,
                    after_if_expr: false,
                    field_of: None,
                },
            },
            Some(syntax::InstantiationReference::Outcome) => PathKind::Expr {
                expr_ctx: ExprCtx {
                    in_block_expr: false,
                    in_condition: false,
                    in_match_guard: false,
                    after_if_expr: false,
                    field_of: None,
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

    // A type position: the name being typed is inside a type the tree
    // has parsed — a `val` signature's parameter, an `option(..)`
    // argument, a tuple element — or nothing is typed yet after `:` or
    // `->`. Only the text check existed, so every type position that did
    // not directly follow `:` or `->` read as the top level of the file and
    // offered item keywords: 288 of 873 type names in sail-riscv.
    if token.is_some_and(in_type) || trimmed.ends_with(':') || trimmed.ends_with("->") {
        return CompletionAnalysis::NameRef(NameRefContext {
            dot_access: None,
            path_ctx: Some(PathCompletionCtx { qualified: Qualified::No, kind: PathKind::Type }),
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
                        field_of: None,
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
                    field_of: None,
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

/// What the position being completed expects, read from the tree: the
/// parameter of the call it is an argument of, or `bool` in a condition.
///
/// This read the text before the cursor, and the commonest answer was
/// wrong: `rfind("->")` found the enclosing function's own return type, so
/// every position in a body expected that type and every function
/// returning it scored higher, while a call argument produced no type at
/// all, only a `"f#1"` that no candidate's name could equal.
fn expected_type_and_name(
    sema: &hir::Semantics<'_>,
    db: &dyn salsa::Database,
    file_text: Option<base_db::FileText>,
    token: Option<&syntax::SyntaxToken>,
) -> (Option<hir::Ty>, Option<String>) {
    use syntax::SyntaxKind as SK;
    let Some(name) = token.and_then(|t| t.parent()).filter(|n| n.kind() == SK::IDENT_EXPR) else {
        return (None, None);
    };
    let Some(parent) = name.parent() else { return (None, None) };
    let is_first = parent.first_child().as_ref() == Some(&name);
    match parent.kind() {
        SK::IF_EXPR | SK::WHILE_EXPR if is_first => {
            (Some(hir::Ty::scalar(hir::Scalar::Bool)), None)
        }
        SK::FIELD_INIT => {
            let Some(ft) = file_text else { return (None, None) };
            let Some(eq) = parent.children_with_tokens().find(|e| e.kind() == SK::EQ) else {
                return (None, None);
            };
            if name.text_range().start() < eq.text_range().end() {
                return (None, None);
            }
            let field = parent
                .children_with_tokens()
                .filter_map(|e| e.into_token())
                .find(|t| t.kind() == SK::IDENT)
                .map(|t| t.text().to_owned());
            let record = parent.parent();
            let ty = record.as_ref().zip(field.as_ref()).and_then(|(record, field)| {
                let at = usize::from(record.text_range().start());
                let owner = sema.type_of_expr(ft, at)?;
                sema.record_field_type(ft, at, &owner, field)
            });
            (ty, field)
        }
        SK::ARG_LIST => {
            let Some(index) = parent.children().position(|n| n == name) else {
                return (None, None);
            };
            let Some(callee) = parent.parent().and_then(|call| call.first_child()) else {
                return (None, None);
            };
            let Some(ft) = file_text else { return (None, None) };
            let at = usize::from(callee.text_range().end()) - 1;
            let ty = match sema.type_of_expr(ft, at).as_ref().map(hir::Ty::kind) {
                Some(hir::TyKind::FnPtr(sig)) => sig.params.get(index).cloned(),
                _ => None,
            };
            let param_name =
                ide_db::signatures::callable_signatures_at(db, ft, at, &callee.text().to_string())
                    .and_then(|sigs| sigs.into_iter().next())
                    .and_then(|sig| sig.params.into_iter().nth(index))
                    .map(|p| p.name.split(':').next().unwrap_or(&p.name).trim().to_owned());
            (ty, param_name)
        }
        _ => (None, None),
    }
}

pub(crate) fn relevance_type_match(
    matched: hir::CompletionTypeMatch,
) -> CompletionRelevanceTypeMatch {
    match matched {
        hir::CompletionTypeMatch::Exact => CompletionRelevanceTypeMatch::Exact,
        hir::CompletionTypeMatch::CouldUnify => CompletionRelevanceTypeMatch::CouldUnify,
    }
}

/// Ask the semantic layer; display labels are never type identities.
pub(crate) fn compute_type_match(
    ctx: &CompletionContext<'_>,
    expected: &hir::Ty,
    candidate: &hir::Ty,
) -> Option<CompletionRelevanceTypeMatch> {
    ctx.sema
        .completion_type_match(ctx.file_text?, ctx.semantic_offset(), expected, candidate)
        .map(relevance_type_match)
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
        self.instantiation_role
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
