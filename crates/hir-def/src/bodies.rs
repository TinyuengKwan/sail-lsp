//! Per-file cache of `(Body, BodySourceMap)` for each callable definition.
//!
//! Built once during parse; enables O(1) cursor-to-ExprId resolution
//! without re-walking the AST on every IDE query.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::body::{Body, BodySourceMap};
use crate::expr_store::hir::Expr;
use crate::Span;

/// Side-effect tag for a callable body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EffectTag {
    /// `throw expr` — escapes via the exception channel.
    Throw,
    /// `exit expr` — terminates the program.
    Exit,
    /// `return expr` — early return.
    Return,
    /// `assert(...)` — runtime assertion check.
    Assert,
    /// Register write: `reg = value`.
    RegisterWrite,
    /// Register read: accessing a register name.
    RegisterRead,
    /// Non-exhaustive match expression.
    IncompleteMatch,
    /// Call to an external (non-pure) function.
    External,
    /// `undefined` literal in expression.
    Undefined,
    /// Scattered function definition.
    Scattered,
    /// Non-executable pattern (e.g. `P_string_append`).
    NonExec,
}

impl EffectTag {
    pub fn as_str(self) -> &'static str {
        match self {
            EffectTag::Throw => "throw",
            EffectTag::Exit => "exit",
            EffectTag::Return => "return",
            EffectTag::Assert => "assert",
            EffectTag::RegisterWrite => "wreg",
            EffectTag::RegisterRead => "rreg",
            EffectTag::IncompleteMatch => "incomplete_match",
            EffectTag::External => "external",
            EffectTag::Undefined => "undefined",
            EffectTag::Scattered => "scattered",
            EffectTag::NonExec => "nonexec",
        }
    }
}

/// One arena per callable body in a file.
#[derive(Clone, Debug, Default)]
pub struct CallableBodies {
    entries: Vec<CallableBody>,
}

/// A single callable's body + source map + metadata.
#[derive(Clone, Debug)]
pub struct CallableBody {
    pub name: String,
    pub def_span: Span,
    #[allow(dead_code)]
    pub body_span: Span,
    pub body: Arc<Body>,
    pub source_map: Arc<BodySourceMap>,
    pub effects: BTreeSet<EffectTag>,
    /// Outcome identifiers (e.g., "Error") -- separate from `EffectTag` to keep it `Copy`.
    pub outcomes: BTreeSet<String>,
}

impl CallableBodies {
    /// Build a cache from a CST root node.
    pub fn from_cst(root: &syntax::SyntaxNode) -> Self {
        use parser::SyntaxKind as SK;
        use syntax::ast::{
            AstNode as _, CallableDef, CallableSpec, NamedDef, ParamList, ScatteredClauseDef,
        };

        // Collect register names from NAMED_DEF nodes for effect detection.
        let register_names: std::collections::HashSet<String> = root
            .children()
            .filter(|c| NamedDef::can_cast(c.kind()))
            .filter(|c| {
                c.descendants_with_tokens()
                    .filter_map(|el| el.into_token())
                    .any(|t| t.kind() == SK::KW_REGISTER)
            })
            .filter_map(|c| {
                c.descendants_with_tokens()
                    .filter_map(|el| el.into_token())
                    .find(|t| t.kind() == SK::IDENT)
                    .map(|t| t.text().to_string())
            })
            .collect();

        // Collect names of functions declared as `pure` in val specs.
        // A val spec like `val foo = pure { c: "c_foo" } : ...` or containing
        // `pure` keyword marks the function as side-effect-free.
        let _pure_functions: std::collections::HashSet<String> = root
            .children()
            .filter(|c| CallableSpec::can_cast(c.kind()))
            .filter(|c| {
                let text = c.text().to_string();
                text.contains(" pure ") || text.contains("= pure")
            })
            .filter_map(|c| {
                c.descendants_with_tokens()
                    .filter_map(|el| el.into_token())
                    .find(|t| t.kind() == SK::IDENT)
                    .map(|t| t.text().to_string())
            })
            .collect();

        let source = root.text().to_string();
        let mut entries = Vec::new();
        for child in root.children() {
            // Process CALLABLE_DEF (regular function/mapping) and
            // SCATTERED_CLAUSE_DEF (function clause/mapping clause).
            // Clauses produce separate callable bodies that share the
            // scattered definition name.
            if !CallableDef::can_cast(child.kind()) && !ScatteredClauseDef::can_cast(child.kind()) {
                continue;
            }
            let range = child.text_range();
            let def_span = Span::new(usize::from(range.start()), usize::from(range.end()));

            // Extract name: first IDENT token after keyword
            let name = Self::cst_callable_name(&child);
            if name.is_empty() {
                continue;
            }

            // Determine if this is a mapping or function
            let is_mapping = child
                .descendants_with_tokens()
                .filter_map(|el| el.into_token())
                .any(|t| t.kind() == SK::KW_MAPPING);

            // Detect scattered clause (function clause / mapping clause)
            let is_scattered_clause = child
                .descendants_with_tokens()
                .filter_map(|el| el.into_token())
                .any(|t| t.kind() == SK::KW_CLAUSE);

            // Find body expression node (child after `=` token)
            if let Some(body_node) = Self::cst_callable_body_node(&child) {
                if is_mapping {
                    // Mapping: lower body and extract mapping arms from <-> infix
                    let (mut body, source_map) = Body::lower_body(&body_node);
                    let bs = Self::trimmed_span(&body_node, &source);
                    Self::extract_mapping_arms_from_body(&mut body, &source, bs);
                    entries.push(CallableBody {
                        name: name.clone(),
                        def_span,
                        body_span: Self::trimmed_span(&body_node, &source),
                        body: Arc::new(body),
                        source_map: Arc::new(source_map),
                        effects: BTreeSet::new(),
                        outcomes: BTreeSet::new(),
                    });
                } else {
                    // Lower body + params in one pass via lower_body_with_params.
                    // during body lowering, not as a post-processing step.
                    let param_list_node = child.children().find(|c| ParamList::can_cast(c.kind()));
                    let (body, source_map) =
                        Body::lower_body_with_params(&body_node, param_list_node.as_ref());
                    let mut effects = collect_effect_tags(&body, &register_names);
                    // Mark scattered clause with Scattered effect
                    if is_scattered_clause {
                        effects.insert(EffectTag::Scattered);
                    }
                    entries.push(CallableBody {
                        name: name.clone(),
                        def_span,
                        body_span: Self::trimmed_span(&body_node, &source),
                        body: Arc::new(body),
                        source_map: Arc::new(source_map),
                        effects,
                        outcomes: BTreeSet::new(),
                    });
                }
                continue;
            }

            // Mappings without `= body` are handled above via the body_node path.
            // If no body_node was found, the mapping has no body to process.
        }
        Self { entries }
    }

    /// Compute body_span from a CST node, trimming trailing whitespace
    /// to match the legacy body_expr span which doesn't include trivia.
    fn trimmed_span(node: &syntax::SyntaxNode, source: &str) -> Span {
        let start = usize::from(node.text_range().start());
        let mut end = usize::from(node.text_range().end());
        // Trim trailing whitespace/newlines
        while end > start {
            match source.as_bytes().get(end - 1) {
                Some(b' ' | b'\n' | b'\r' | b'\t') => end -= 1,
                _ => break,
            }
        }
        Span::new(start, end)
    }

    /// Extract callable name from a CST CALLABLE_DEF node.
    fn cst_callable_name(node: &syntax::SyntaxNode) -> String {
        use parser::SyntaxKind as SK;
        // Skip keywords, find first IDENT
        for tok in node.descendants_with_tokens().filter_map(|el| el.into_token()) {
            if tok.kind() == SK::IDENT {
                return tok.text().to_string();
            }
        }
        String::new()
    }

    /// Find the body expression node after `=` in a CALLABLE_DEF.
    pub fn cst_callable_body_node(node: &syntax::SyntaxNode) -> Option<syntax::SyntaxNode> {
        use parser::SyntaxKind as SK;
        let mut found_eq = false;
        for element in node.children_with_tokens() {
            match element {
                rowan::NodeOrToken::Token(tok) if tok.kind() == SK::EQ => {
                    found_eq = true;
                }
                rowan::NodeOrToken::Node(child) if found_eq => {
                    return Some(child);
                }
                _ => {}
            }
        }
        None
    }

    /// Extract parameter PatIds from PARAM_LIST, adding them to the body arena.
    // extract_cst_params deleted — replaced by Body::lower_body_with_params
    // which uses ExprCollector::lower_cst_pat for structural pattern lowering.

    /// Try to extract a mapping body from a CALLABLE_DEF CST node.
    /// Scan a lowered Body for `<->` infix expressions and convert them
    /// into MappingArm entries. Used when the body was lowered from
    /// a mapping definition's CST.
    fn extract_mapping_arms_from_body(body: &mut Body, source: &str, body_span: Span) {
        use crate::expr_store::hir::{Expr, MappingArm, MappingDirection, Pat};

        let root = body.body_expr;
        // Collect <-> infix expressions from the root (which may be a Block)
        let mut arm_expr_ids = Vec::new();
        match body.expr(root) {
            Some(Expr::Block(stmts)) => {
                for stmt in stmts.clone() {
                    if let crate::expr_store::hir::Statement::Expr(eid) = stmt {
                        arm_expr_ids.push(eid);
                    }
                }
            }
            Some(Expr::BinaryOp { .. }) => {
                arm_expr_ids.push(root);
            }
            _ => {}
        }

        // Also check for direction keywords (backwards/forwards) in source.
        // These arms don't use <-> but: `backwards pat [if guard] => body`
        // The CST parser may not produce structured exprs for these, so
        // scan the body source text directly.
        let body_text = source.get(body_span.start..body_span.end).unwrap_or("");
        // Strip outer braces if present
        let inner = body_text.trim();
        let inner = inner.strip_prefix('{').unwrap_or(inner);
        let inner = inner.strip_suffix('}').unwrap_or(inner).trim();
        // Split by comma to get individual arms
        for arm_text_raw in inner.split(',') {
            let arm_text = arm_text_raw.trim();
            let trimmed = arm_text.trim();
            let direction = if trimmed.starts_with("backwards") {
                Some(MappingDirection::Backwards)
            } else if trimmed.starts_with("forwards") {
                Some(MappingDirection::Forwards)
            } else {
                None
            };
            if let Some(dir) = direction {
                // Extract pattern + optional guard + body from remaining text.
                // This is a best-effort extraction since the CST doesn't
                // structure mapping-specific arms.
                let rest = trimmed
                    .strip_prefix("backwards")
                    .or_else(|| trimmed.strip_prefix("forwards"))
                    .unwrap_or("")
                    .trim();
                // Find `=>` to split guard+pattern from body
                if let Some(arrow_pos) = rest.find("=>") {
                    let pat_guard = rest[..arrow_pos].trim();
                    let _body_text = rest[arrow_pos + 2..].trim();

                    // Split pattern and guard at `if`
                    let (pat_text, guard_text) = if let Some(if_pos) = pat_guard.find(" if ") {
                        (&pat_guard[..if_pos], Some(&pat_guard[if_pos + 4..]))
                    } else {
                        (pat_guard, None)
                    };

                    // Create pattern binding
                    let pat_name: String = pat_text.trim().to_string();
                    let pid = body.store.pats.alloc(Pat::Bind(pat_name));
                    let pat_id = Some(pid);

                    // Create placeholder expr for guard
                    let placeholder = body.body_expr;
                    let guard: Option<crate::body::ExprId> = guard_text.map(|_| placeholder);

                    body.mapping_arms.push(MappingArm {
                        direction: dir,
                        lhs_pat: pat_id,
                        rhs_pat: None,
                        lhs_expr: placeholder,
                        rhs_expr: placeholder,
                        guard,
                        span: body_span,
                    });
                }
            }
        }

        for eid in arm_expr_ids {
            let (lhs_id, rhs_id, span) = match body.expr(eid) {
                Some(Expr::BinaryOp { lhs, op, rhs }) if op.as_str() == "<->" => {
                    (*lhs, *rhs, body_span)
                }
                _ => continue,
            };
            // Convert lhs/rhs expressions to patterns (best-effort: Bind for idents)
            let lhs_pat = Self::expr_to_pat(body, lhs_id);
            let rhs_pat = Self::expr_to_pat(body, rhs_id);
            body.mapping_arms.push(MappingArm {
                direction: MappingDirection::Bidirectional,
                lhs_pat,
                rhs_pat,
                lhs_expr: lhs_id,
                rhs_expr: rhs_id,
                guard: None,
                span,
            });
        }
    }

    /// Best-effort conversion of an ExprId to a PatId for mapping arms.
    fn expr_to_pat(body: &mut Body, expr_id: crate::body::ExprId) -> Option<crate::body::PatId> {
        use crate::expr_store::hir::{Expr, Pat};
        match body.expr(expr_id) {
            Some(Expr::Ident(name)) => {
                let name = name.clone();
                let id = body.store.pats.alloc(Pat::Bind(name));
                Some(id)
            }
            Some(Expr::Literal(lit)) => {
                let lit = lit.clone();
                let id = body.store.pats.alloc(Pat::Literal(lit));
                Some(id)
            }
            // Constructor pattern: ADD(a, b) → App { ctor: "ADD", args: [Bind("a"), Bind("b")] }
            Some(Expr::Call { callee, args }) => {
                let callee = *callee;
                let args = args.clone();
                let ctor_name = match body.expr(callee) {
                    Some(Expr::Ident(n)) => n.clone(),
                    _ => return None,
                };
                let pat_args: Vec<_> = args
                    .iter()
                    .filter_map(|a| Self::expr_to_pat(body, *a))
                    .collect();
                let id = body.store.pats.alloc(Pat::App { ctor: ctor_name, args: pat_args });
                Some(id)
            }
            // Tuple: (a, b) → Tuple([Bind("a"), Bind("b")])
            Some(Expr::Tuple(items)) => {
                let items = items.clone();
                let pat_items: Vec<_> = items
                    .iter()
                    .filter_map(|e| Self::expr_to_pat(body, *e))
                    .collect();
                let id = body.store.pats.alloc(Pat::Tuple(pat_items));
                Some(id)
            }
            // Cast: expr : type → Typed { inner }
            Some(Expr::Cast { expr, .. }) => {
                let expr = *expr;
                Self::expr_to_pat(body, expr)
            }
            // Binary op (@, ::) → Infix
            Some(Expr::BinaryOp { lhs, op, rhs }) => {
                let lhs = *lhs;
                let op = op.clone();
                let rhs = *rhs;
                let lhs_pat = Self::expr_to_pat(body, lhs)?;
                let rhs_pat = Self::expr_to_pat(body, rhs)?;
                let id = body.store.pats.alloc(Pat::Infix { lhs: lhs_pat, op: op.to_string(), rhs: rhs_pat });
                Some(id)
            }
            _ => None,
        }
    }

    /// Number of cached callable bodies.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True iff no callable bodies were cached.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// All entries in source order.
    #[allow(dead_code)]
    pub fn entries(&self) -> &[CallableBody] {
        &self.entries
    }

    /// Build from pre-computed entries (for testing).
    pub fn from_entries(entries: Vec<(String, Span, BTreeSet<EffectTag>)>) -> Self {
        use crate::body::BodySourceMap;
        use crate::expr_store::hir::Expr;
        use crate::expr_store::ExpressionStoreBuilder;

        let bodies: Vec<CallableBody> = entries
            .into_iter()
            .map(|(name, span, effects)| {
                let mut builder = ExpressionStoreBuilder::new();
                let id = builder.alloc_expr(Expr::Missing, Span::new(0, 0));
                let (store, expr_source_map) = builder.finish();
                let body = Body::new(store, vec![], id);
                let source_map = BodySourceMap { file_id: None, store: expr_source_map };
                CallableBody {
                    name,
                    def_span: span,
                    body_span: span,
                    body: Arc::new(body),
                    source_map: Arc::new(source_map),
                    effects,
                    outcomes: BTreeSet::new(),
                }
            })
            .collect();
        Self { entries: bodies }
    }

    /// Entries matching `name` (may be multiple for function clauses).
    #[allow(dead_code)]
    pub fn by_name<'a, 'b>(
        &'a self,
        name: &'b str,
    ) -> impl Iterator<Item = &'a CallableBody> + use<'a, 'b> {
        self.entries.iter().filter(move |e| e.name == name)
    }

    /// Smallest callable whose `def_span` covers `offset`.
    pub fn entry_at_offset(&self, offset: usize) -> Option<&CallableBody> {
        let mut best: Option<(usize, &CallableBody)> = None;
        for entry in &self.entries {
            let span = entry.def_span;
            if span.start <= offset && offset < span.end {
                let width = span.end - span.start;
                if best.map(|(w, _)| w > width).unwrap_or(true) {
                    best = Some((width, entry));
                }
            }
        }
        best.map(|(_, e)| e)
    }
}

/// Walk a body and collect structural side effects (throw, exit, etc.).
fn collect_effect_tags(
    body: &Body,
    register_names: &std::collections::HashSet<String>,
) -> BTreeSet<EffectTag> {
    let mut tags = BTreeSet::new();
    for (_, hir) in body.iter_exprs() {
        match hir {
            Expr::Throw(_) => {
                tags.insert(EffectTag::Throw);
            }
            Expr::Exit(_) => {
                tags.insert(EffectTag::Exit);
            }
            Expr::Return(_) => {
                tags.insert(EffectTag::Return);
            }
            Expr::Assert { .. } => {
                tags.insert(EffectTag::Assert);
            }
            Expr::Literal(parser::Literal::Undefined) => {
                tags.insert(EffectTag::Undefined);
            }
            Expr::Ident(name) if register_names.contains(name.as_str()) => {
                tags.insert(EffectTag::RegisterRead);
            }
            Expr::Assign { target, .. } => {
                if hir_is_register_write(body, *target) {
                    tags.insert(EffectTag::RegisterWrite);
                }
            }
            // Detect calls to known-impure external functions.
            // We only tag External for functions in the local file that have
            // observable structural effects. Cross-file effect inference is done
            // by WorkspaceEffects in hir-ty, not here.
            Expr::Call { .. } => {
                // Don't tag External here — per-file structural analysis cannot
                // determine cross-file function purity. Tagging every unknown
                // call as External causes massive false positives (445+ on
                // sail-riscv). Defer to workspace-level transitive effect
                // propagation in hir-ty/infer/expr.rs.
            }
            _ => {}
        }
    }
    tags
}

/// Heuristic: check if an assign target looks like a register write.
fn hir_is_register_write(body: &Body, target: crate::body::ExprId) -> bool {
    match body.expr(target) {
        Some(Expr::Field { expr: inner, .. }) => {
            // field.x = ... — register write if inner is an Ident
            matches!(body.expr(*inner), Some(Expr::Ident(_)) | Some(Expr::Field { .. }))
                || hir_is_register_write(body, *inner)
        }
        // Deref lexps lower to their inner expression directly
        Some(Expr::Ident(_)) => false, // bare ident — could be local, can't tell
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bodies(source: &str) -> CallableBodies {
        let (root, _) = syntax::parse_text(source);
        CallableBodies::from_cst(&root)
    }

    #[test]
    fn empty_file_has_no_bodies() {
        let bodies = bodies("");
        assert_eq!(bodies.len(), 0);
        assert!(bodies.is_empty());
    }

    #[test]
    fn val_spec_alone_yields_no_body() {
        let bodies = bodies("val foo : int -> int\n");
        assert!(bodies.is_empty());
    }

    #[test]
    fn function_definition_yields_one_body() {
        let bodies = bodies("val foo : int -> int\nfunction foo(x) = x + 1\n");
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies.entries()[0].name, "foo");
        // Body has 3 nodes: Infix root + Ident x + Literal 1.
        assert_eq!(bodies.entries()[0].body.len(), 3);
    }

    #[test]
    fn multiple_function_clauses_share_a_name() {
        let source = "\
val pick : int -> int
function clause pick(0) = 100
function clause pick(n) = n
";
        let bodies = bodies(source);
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies.by_name("pick").count(), 2);
    }

    #[test]
    fn entry_at_offset_finds_containing_callable() {
        let source = "\
function add(x, y) = x + y
function mul(x, y) = x * y
";
        let bodies = bodies(source);
        // Cursor inside `add` body — pick `add`.
        let add_body_offset = source.find("x + y").unwrap();
        let add = bodies.entry_at_offset(add_body_offset).expect("add");
        assert_eq!(add.name, "add");
        // Cursor inside `mul` body — pick `mul`.
        let mul_body_offset = source.find("x * y").unwrap();
        let mul = bodies.entry_at_offset(mul_body_offset).expect("mul");
        assert_eq!(mul.name, "mul");
    }

    #[test]
    fn entry_at_offset_returns_none_outside_any_body() {
        let source = "function f() = ()\n\nval g : int\n";
        let bodies = bodies(source);
        // Offset on the `val g` line is outside any callable body.
        let val_offset = source.find("val g").unwrap();
        assert!(bodies.entry_at_offset(val_offset).is_none());
    }

    #[test]
    fn pure_function_has_no_effect_tags() {
        let bodies = bodies("function add(x : int, y : int) -> int = x + y\n");
        assert_eq!(bodies.entries().len(), 1);
        assert!(bodies.entries()[0].effects.is_empty());
    }

    #[test]
    fn throw_expression_records_throw_effect() {
        let bodies =
            bodies("function check(x : int) -> int = if x == 0 then throw(\"zero\") else x\n");
        let tags = &bodies.entries()[0].effects;
        assert!(tags.contains(&EffectTag::Throw));
        assert!(!tags.contains(&EffectTag::Exit));
    }

    #[test]
    fn exit_expression_records_exit_effect() {
        let bodies = bodies("function bail() -> unit = exit()\n");
        let tags = &bodies.entries()[0].effects;
        assert!(tags.contains(&EffectTag::Exit));
    }

    #[test]
    fn assert_expression_records_assert_effect() {
        let bodies = bodies("function check(x : int) -> int = { assert(x > 0); x }\n");
        let tags = &bodies.entries()[0].effects;
        assert!(tags.contains(&EffectTag::Assert));
    }

    #[test]
    fn return_expression_records_return_effect() {
        let bodies = bodies("function early(x : int) -> int = if x == 0 then return 0 else x\n");
        let tags = &bodies.entries()[0].effects;
        assert!(tags.contains(&EffectTag::Return));
    }

    #[test]
    #[allow(clippy::print_stderr)]
    fn typed_params_produce_typed_patterns() {
        let source = "function f(x : bits(3), y : bits(2)) = x\n";
        let bodies = bodies(source);
        let entry = &bodies.entries()[0];
        eprintln!("params: {:?}", entry.body.params);
        for &pid in entry.body.params.iter() {
            let pat = entry.body.pat(pid);
            eprintln!("  pat {:?}: {:?}", pid, pat);
        }
        // At least one param should be Typed
        assert!(
            entry.body.params.iter().any(|&pid| {
                matches!(entry.body.pat(pid), Some(crate::expr_store::hir::Pat::Typed { .. }))
            }),
            "expected at least one Typed pattern for typed params"
        );
    }

    #[test]
    fn body_source_map_resolves_subexpression_at_cursor() {
        let source = "function add(x, y) = x + y\n";
        let bodies = bodies(source);
        let entry = &bodies.entries()[0];
        // Cursor on `x` — should resolve to the Ident(x) ExprId,
        // not the surrounding Infix.
        let x_offset = source.find("x + y").unwrap();
        let id = entry.source_map.expr_at_offset(x_offset).expect("hit");
        // The smallest enclosing expression at that offset is the
        // Ident `x`; verify by looking up its span.
        let _resolved = entry.body.expr(id).expect("ident in arena");
        let resolved_span = entry.source_map.expr_syntax(id).expect("span");
        let resolved_text = &source[resolved_span.start..resolved_span.end];
        assert_eq!(resolved_text, "x");
    }

    #[test]
    fn cst_function_produces_body() {
        let b = bodies("function add(x, y) = x + y\n");
        assert_eq!(b.len(), 1);
        assert_eq!(b.entries()[0].name, "add");
    }

    #[test]
    fn cst_claused_function() {
        let b = bodies(
            "val pick : int -> int\nfunction clause pick(0) = 100\nfunction clause pick(n) = n\n",
        );
        assert_eq!(b.len(), 2);
        assert_eq!(b.by_name("pick").count(), 2);
    }

    #[test]
    fn cst_params_are_bound() {
        let b = bodies("function f(x, y) = x + y\n");
        assert_eq!(b.len(), 1);
        assert_eq!(b.entries()[0].body.params.len(), 2);
    }

    #[test]
    fn cst_mapping_produces_body() {
        let b = bodies("mapping size_bits : int <-> bool = { 1 <-> true, 0 <-> false }\n");
        assert!(b.len() >= 1, "mapping should produce at least 1 body");
    }

    #[test]
    fn cst_full_file() {
        let b = bodies(
            "\
val f : int -> int
function f(x) = x + 1
val g : bool -> unit
function g(b) = if b then () else ()
",
        );
        assert_eq!(b.len(), 2);
        assert_eq!(b.entries()[0].name, "f");
        assert_eq!(b.entries()[1].name, "g");
    }

    #[test]
    fn undefined_literal_detected_as_effect() {
        let b = bodies("function foo() = undefined\n");
        assert_eq!(b.len(), 1);
        assert!(
            b.entries()[0].effects.contains(&EffectTag::Undefined),
            "should detect Undefined effect, got: {:?}",
            b.entries()[0].effects
        );
    }

    #[test]
    fn throw_detected_as_effect() {
        let b = bodies("function bar() = throw(\"error\")\n");
        assert_eq!(b.len(), 1);
        assert!(
            b.entries()[0].effects.contains(&EffectTag::Throw),
            "should detect Throw effect, got: {:?}",
            b.entries()[0].effects
        );
    }

    #[test]
    fn no_effects_on_pure_function() {
        let b = bodies("function pure_fn(x) = x + 1\n");
        assert_eq!(b.len(), 1);
        assert!(
            b.entries()[0].effects.is_empty(),
            "pure function should have no effects, got: {:?}",
            b.entries()[0].effects
        );
    }
}
