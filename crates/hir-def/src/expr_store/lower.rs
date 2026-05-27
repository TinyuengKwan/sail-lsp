//! CST → HIR expression/pattern lowering.
//! `ExprCollector` walks the rowan CST and builds `Expr`/`Pat` arenas
//! via `ExpressionStoreBuilder`. The entry points are `lower_body()`
//! and `lower_body_with_params()` on `Body`.

use crate::expr_store::body::{Body, BodySourceMap, ExprId, PatId};
use crate::expr_store::hir::{Expr, HirBinaryOp, HirUnaryOp, Pat};
use crate::expr_store::ExpressionStoreBuilder;
use crate::Span;
use syntax::ast::{self, AstNode as _};

/// Lower a CST node into a Body + BodySourceMap.
pub fn lower_body(root: &syntax::SyntaxNode) -> (Body, BodySourceMap) {
    lower_body_with_params(root, None)
}

/// Lower body expression AND parameter patterns in one pass.
pub fn lower_body_with_params(
    root: &syntax::SyntaxNode,
    param_list_node: Option<&syntax::SyntaxNode>,
) -> (Body, BodySourceMap) {
    use parser::SyntaxKind as SK;

    let mut ctx = ExprCollector::new();

    let params: Vec<PatId> = if let Some(pl) = param_list_node {
        // First try child NODES (structured patterns like IDENT_PAT, APP_PAT, etc.)
        let from_nodes: Vec<PatId> = pl
            .children()
            .filter(|c| {
                let k = c.kind();
                ast::Pat::can_cast(k) || ast::IdentExpr::can_cast(k)
            })
            .map(|c| ctx.lower_cst_pat(&c))
            .collect();

        if !from_nodes.is_empty() {
            from_nodes
        } else {
            // Fallback: extract params from raw tokens/nodes in PARAM_LIST.
            // PARAM_LIST contains tokens (IDENT, COMMA, COLON, parens) and
            // TYPE_* nodes from parsed type annotations.
            // Pattern: IDENT [COLON TYPE_*] → Pat::Bind or Pat::Typed
            use rowan::NodeOrToken;
            let elements: Vec<_> = pl.children_with_tokens().collect();
            let mut params = Vec::new();
            let mut i = 0;
            while i < elements.len() {
                match &elements[i] {
                    NodeOrToken::Token(tok) => {
                        let k = tok.kind();
                        if k == SK::IDENT || k == SK::UNDERSCORE {
                            let name = tok.text().to_string();
                            // Look ahead: skip whitespace, check for COLON + TYPE_* node
                            let mut j = i + 1;
                            while j < elements.len() {
                                if let NodeOrToken::Token(t) = &elements[j] {
                                    if t.kind() == SK::WHITESPACE {
                                        j += 1;
                                        continue;
                                    }
                                }
                                break;
                            }
                            if j < elements.len() {
                                if let NodeOrToken::Token(t) = &elements[j] {
                                    if t.kind() == SK::COLON {
                                        // Skip colon and whitespace, find TYPE node
                                        j += 1;
                                        while j < elements.len() {
                                            if let NodeOrToken::Token(t) = &elements[j] {
                                                if t.kind() == SK::WHITESPACE {
                                                    j += 1;
                                                    continue;
                                                }
                                            }
                                            break;
                                        }
                                        if j < elements.len() {
                                            if let NodeOrToken::Node(type_node) = &elements[j] {
                                                if type_node.kind().is_type_node() {
                                                    // Typed param: create Pat::Typed
                                                    let ty_start =
                                                        usize::from(type_node.text_range().start());
                                                    let ty_end =
                                                        usize::from(type_node.text_range().end());
                                                    let inner_pat = ctx.alloc_pat_bind(name);
                                                    let typed = Pat::Typed {
                                                        inner: inner_pat,
                                                        ty_span: Span {
                                                            start: ty_start,
                                                            end: ty_end,
                                                        },
                                                    };
                                                    params.push(ctx.alloc_pat(
                                                        typed,
                                                        Span { start: 0, end: 0 },
                                                    ));
                                                    i = j + 1;
                                                    continue;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            // No type annotation: plain bind
                            params.push(ctx.alloc_pat_bind(name));
                        }
                    }
                    NodeOrToken::Node(node) => {
                        let k = node.kind();
                        if ast::Pat::can_cast(k) || ast::IdentExpr::can_cast(k) {
                            params.push(ctx.lower_cst_pat(node));
                        }
                        // Skip TYPE_* nodes (consumed as part of typed param above)
                    }
                }
                i += 1;
            }
            params
        }
    } else {
        Vec::new()
    };

    let body_expr = ctx.lower_cst_expr(root);
    let (store, store_source_map) = ctx.builder.finish();

    let body =
        Body { store, params: params.into_boxed_slice(), body_expr, mapping_arms: Vec::new() };
    let source_map = BodySourceMap { file_id: None, store: store_source_map };
    (body, source_map)
}

/// Accumulates HIR expressions and patterns during CST lowering.
pub(crate) struct ExprCollector {
    pub(crate) builder: ExpressionStoreBuilder,
}

impl ExprCollector {
    pub(crate) fn new() -> Self {
        Self { builder: ExpressionStoreBuilder::new() }
    }

    fn alloc_expr(&mut self, hir: Expr, span: Span) -> ExprId {
        self.builder.alloc_expr(hir, span)
    }

    fn alloc_expr_cst(&mut self, hir: Expr, node: &syntax::SyntaxNode) -> ExprId {
        self.builder.alloc_expr_cst(hir, node)
    }

    pub(crate) fn alloc_pat(&mut self, hir: Pat, span: Span) -> PatId {
        let name = Self::binding_name_of_pat(&hir);
        let pat_id = self.builder.alloc_pat(hir, span);
        if let Some(n) = name {
            self.builder.alloc_binding(n, pat_id);
        }
        pat_id
    }

    fn alloc_pat_cst(&mut self, hir: Pat, node: &syntax::SyntaxNode) -> PatId {
        let name = Self::binding_name_of_pat(&hir);
        let pat_id = self.builder.alloc_pat_cst(hir, node);
        if let Some(n) = name {
            self.builder.alloc_binding(n, pat_id);
        }
        pat_id
    }

    /// Allocate a Pat::Bind from a raw token name (used when PARAM_LIST
    /// contains only tokens without wrapping pattern nodes).
    pub(crate) fn alloc_pat_bind(&mut self, name: String) -> PatId {
        let hir = Pat::Bind(name);
        self.alloc_pat(hir, Span { start: 0, end: 0 })
    }

    /// Extract binding name from a pattern, if it introduces a name binding.
    fn binding_name_of_pat(hir: &Pat) -> Option<crate::name::Name> {
        match hir {
            Pat::Bind(name) => Some(crate::name::Name::new(name)),
            Pat::TypeVar(name) => Some(crate::name::Name::new(name)),
            Pat::As { binding, .. } => Some(crate::name::Name::new(binding)),
            _ => None,
        }
    }

    fn text_range_to_span(range: rowan::TextRange) -> Span {
        Span::new(usize::from(range.start()), usize::from(range.end()))
    }

    pub(crate) fn lower_cst_expr(&mut self, node: &syntax::SyntaxNode) -> ExprId {
        use parser::SyntaxKind as SK;

        let span = Self::text_range_to_span(node.text_range());
        let hir = match node.kind() {
            SK::LITERAL_EXPR => {
                let text = node.text().to_string().trim().to_string();
                self.literal_from_text(&text)
            }
            SK::IDENT_EXPR => {
                let text = self
                    .first_ident_text(node)
                    .unwrap_or_else(|| node.text().to_string().trim().to_string());
                Expr::Ident(text)
            }
            SK::TYVAR_EXPR => {
                let text = node.text().to_string().trim().to_string();
                Expr::TypeVar(text)
            }
            SK::REF_EXPR => {
                let text = self.first_ident_text(node).unwrap_or_default();
                Expr::Ref(text)
            }
            SK::BIN_EXPR => {
                let children: Vec<_> = node.children().collect();
                if children.len() >= 2 {
                    let lhs = self.lower_cst_expr(&children[0]);
                    let op_text = self.infix_op_text(node);
                    let op = HirBinaryOp::from_str(&op_text);
                    let rhs = self.lower_cst_expr(&children[children.len() - 1]);
                    Expr::BinaryOp { lhs, op, rhs }
                } else {
                    Expr::Error { message: "incomplete infix expression".to_string() }
                }
            }
            SK::PREFIX_EXPR => {
                let op_text = self.first_non_trivia_token_text(node);
                let op = HirUnaryOp::from_str(&op_text);
                let child = node.children().next();
                let expr = child
                    .map(|c| self.lower_cst_expr(&c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                Expr::UnaryOp { op, expr }
            }
            SK::CALL_EXPR => {
                let children: Vec<_> = node.children().collect();
                if let Some(callee_node) = children.first() {
                    let callee = self.lower_cst_expr(callee_node);
                    let mut args = Vec::new();
                    for child in children.iter().skip(1) {
                        if ast::ArgList::can_cast(child.kind()) {
                            for arg_node in child.children() {
                                args.push(self.lower_cst_expr(&arg_node));
                            }
                        }
                    }
                    Expr::Call { callee, args }
                } else {
                    Expr::Missing
                }
            }
            SK::IF_EXPR => {
                let children: Vec<_> = node.children().collect();
                let cond = children
                    .first()
                    .map(|c| self.lower_cst_expr(c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                let then_branch = children
                    .get(1)
                    .map(|c| self.lower_cst_expr(c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                let else_branch = children.get(2).map(|c| self.lower_cst_expr(c));
                Expr::If { cond, then_branch, else_branch }
            }
            SK::MATCH_EXPR => {
                let children: Vec<_> = node.children().collect();
                let scrutinee = children
                    .first()
                    .map(|c| self.lower_cst_expr(c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                let arms: Vec<_> = node
                    .children()
                    .filter(|c| ast::MatchArm::can_cast(c.kind()))
                    .map(|arm| self.lower_cst_match_arm(&arm))
                    .collect();
                Expr::Match { scrutinee, arms }
            }
            SK::TRY_EXPR => {
                let children: Vec<_> = node.children().collect();
                let scrutinee = children
                    .first()
                    .map(|c| self.lower_cst_expr(c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                let arms: Vec<_> = node
                    .children()
                    .filter(|c| ast::MatchArm::can_cast(c.kind()))
                    .map(|arm| self.lower_cst_match_arm(&arm))
                    .collect();
                Expr::Try { scrutinee, arms }
            }
            SK::BLOCK_EXPR => {
                let mut stmts = Vec::new();
                for c in node.children().filter(|c| ast::BlockItem::can_cast(c.kind())) {
                    self.lower_cst_block_item_into(&c, &mut stmts);
                }
                Expr::Block(stmts)
            }
            SK::LET_EXPR => {
                let children: Vec<_> = node.children().collect();
                let pat = children
                    .first()
                    .map(|c| self.lower_cst_pat(c))
                    .unwrap_or_else(|| self.alloc_pat(Pat::Missing, span));
                let value = children
                    .get(1)
                    .map(|c| self.lower_cst_expr(c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                let body = children
                    .get(2)
                    .map(|c| self.lower_cst_expr(c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                Expr::Let { pat, value, body }
            }
            SK::RETURN_EXPR => {
                let child = node.children().next();
                let expr = child
                    .map(|c| self.lower_cst_expr(&c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                Expr::Return(expr)
            }
            SK::THROW_EXPR => {
                let child = node.children().next();
                let expr = child
                    .map(|c| self.lower_cst_expr(&c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                Expr::Throw(expr)
            }
            SK::EXIT_EXPR => {
                let child = node.children().next().map(|c| self.lower_cst_expr(&c));
                Expr::Exit(child)
            }
            SK::ASSERT_EXPR => {
                let children: Vec<_> = node.children().collect();
                if let Some(first) = children.first() {
                    if ast::TupleExpr::can_cast(first.kind()) {
                        let inner: Vec<_> = first.children().collect();
                        let cond = inner
                            .first()
                            .map(|c| self.lower_cst_expr(c))
                            .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                        let message = inner.get(1).map(|c| self.lower_cst_expr(c));
                        Expr::Assert { cond, message }
                    } else {
                        let cond = self.lower_cst_expr(first);
                        let message = children.get(1).map(|c| self.lower_cst_expr(c));
                        Expr::Assert { cond, message }
                    }
                } else {
                    Expr::Assert { cond: self.alloc_expr(Expr::Missing, span), message: None }
                }
            }
            SK::WHILE_EXPR => {
                let children: Vec<_> = node.children().collect();
                let cond = children
                    .first()
                    .map(|c| self.lower_cst_expr(c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                let body = children
                    .get(1)
                    .map(|c| self.lower_cst_expr(c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                Expr::While { cond, body }
            }
            SK::REPEAT_EXPR => {
                let children: Vec<_> = node.children().collect();
                let body = children
                    .first()
                    .map(|c| self.lower_cst_expr(c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                let until = children
                    .get(1)
                    .map(|c| self.lower_cst_expr(c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                Expr::Repeat { body, until }
            }
            SK::TUPLE_EXPR => {
                let items: Vec<_> = node.children().map(|c| self.lower_cst_expr(&c)).collect();
                // Single-element "tuple" is just grouping parens: (expr) ≡ expr.
                if items.len() == 1 {
                    return items.into_iter().next().unwrap();
                }
                Expr::Tuple(items)
            }
            SK::LIST_EXPR => {
                let items: Vec<_> = node.children().map(|c| self.lower_cst_expr(&c)).collect();
                Expr::List(items)
            }
            SK::VECTOR_EXPR => {
                let items: Vec<_> = node.children().map(|c| self.lower_cst_expr(&c)).collect();
                Expr::Array(items)
            }
            SK::FIELD_ACCESS_EXPR => {
                // `Field(Config([..]), name)` is merged into
                // `Config([.., name])` to produce a single config path.
                let child = node.children().next();
                let field = self.last_ident_text(node).unwrap_or_default();
                let base_expr = child
                    .map(|c| self.lower_cst_expr(&c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                // Check if base is a Config — if so, extend its path.
                let base_is_config = matches!(&self.builder.exprs[base_expr], Expr::Config(_));
                if base_is_config {
                    let segments = match self.builder.exprs[base_expr].clone() {
                        Expr::Config(s) => s,
                        _ => unreachable!(),
                    };
                    let mut extended = segments;
                    extended.push(field);
                    Expr::Config(extended)
                } else {
                    Expr::Field { expr: base_expr, field }
                }
            }
            SK::INDEX_EXPR => {
                let children: Vec<_> = node.children().collect();
                let base = children
                    .first()
                    .map(|c| self.lower_cst_expr(c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                if children.len() >= 3 {
                    let offset = self.lower_cst_expr(&children[1]);
                    let length = self.lower_cst_expr(&children[2]);
                    let callee = self.alloc_expr(Expr::Ident("slice".to_string()), span);
                    Expr::Call { callee, args: vec![base, offset, length] }
                } else {
                    let index = children
                        .get(1)
                        .map(|c| self.lower_cst_expr(c))
                        .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                    Expr::Index { base, index }
                }
            }
            SK::SUBRANGE_EXPR => {
                let children: Vec<_> = node.children().collect();
                let base = children
                    .first()
                    .map(|c| self.lower_cst_expr(c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                let hi = children
                    .get(1)
                    .map(|c| self.lower_cst_expr(c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                let lo = children
                    .get(2)
                    .map(|c| self.lower_cst_expr(c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                Expr::Subrange { base, hi, lo }
            }
            SK::SIZEOF_EXPR => {
                // Extract the nexp text from inside sizeof(...).
                // The CST has: KW_SIZEOF L_PAREN <nexp tokens> R_PAREN
                let nexp_text = node
                    .children_with_tokens()
                    .filter_map(|el| el.into_token())
                    .filter(|t| {
                        !t.kind().is_trivia()
                            && t.kind() != SK::KW_SIZEOF
                            && t.kind() != SK::L_PAREN
                            && t.kind() != SK::R_PAREN
                    })
                    .map(|t| t.text().to_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                let nexp = if nexp_text.trim().is_empty() {
                    node.text().to_string()
                } else {
                    nexp_text.trim().to_string()
                };
                Expr::SizeOf { span, nexp }
            }
            SK::CONSTRAINT_EXPR => Expr::Constraint(span),
            SK::VECTOR_UPDATE_EXPR => {
                let children: Vec<_> = node.children().collect();
                let base = children
                    .first()
                    .map(|c| self.lower_cst_expr(c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                match children.len() {
                    4 => {
                        let hi = self.lower_cst_expr(&children[1]);
                        let lo = self.lower_cst_expr(&children[2]);
                        let value = self.lower_cst_expr(&children[3]);
                        let callee = self
                            .alloc_expr(Expr::Ident("vector_update_subrange#".to_string()), span);
                        Expr::Call { callee, args: vec![base, hi, lo, value] }
                    }
                    3 => {
                        let idx = self.lower_cst_expr(&children[1]);
                        let value = self.lower_cst_expr(&children[2]);
                        let callee =
                            self.alloc_expr(Expr::Ident("vector_update#".to_string()), span);
                        Expr::Call { callee, args: vec![base, idx, value] }
                    }
                    _ => Expr::Ident("__vector_update_error".to_string()),
                }
            }
            SK::FOREACH_EXPR => {
                // Create Pat::Bind for the iterator variable.
                let iterator_name = self.first_ident_text(node).unwrap_or_default();
                let iter_span = node
                    .children_with_tokens()
                    .filter_map(|el| el.into_token())
                    .find(|t| t.kind() == SK::IDENT)
                    .map(|t| {
                        let r = t.text_range();
                        Span::new(usize::from(r.start()), usize::from(r.end()))
                    })
                    .unwrap_or(span);
                let pat = self.alloc_pat(Pat::Bind(iterator_name), iter_span);
                let expr_children: Vec<_> = node.children().collect();
                let (start, end, step, body) = match expr_children.len() {
                    0 => {
                        let m = self.alloc_expr(Expr::Missing, span);
                        (
                            m,
                            self.alloc_expr(Expr::Missing, span),
                            None,
                            self.alloc_expr(Expr::Missing, span),
                        )
                    }
                    1 => {
                        let b = self.lower_cst_expr(&expr_children[0]);
                        let m = self.alloc_expr(Expr::Missing, span);
                        (m, self.alloc_expr(Expr::Missing, span), None, b)
                    }
                    2 => {
                        let s = self.lower_cst_expr(&expr_children[0]);
                        let b = self.lower_cst_expr(&expr_children[1]);
                        (s, self.alloc_expr(Expr::Missing, span), None, b)
                    }
                    3 => {
                        let s = self.lower_cst_expr(&expr_children[0]);
                        let e = self.lower_cst_expr(&expr_children[1]);
                        let b = self.lower_cst_expr(&expr_children[2]);
                        (s, e, None, b)
                    }
                    _ => {
                        let s = self.lower_cst_expr(&expr_children[0]);
                        let e = self.lower_cst_expr(&expr_children[1]);
                        let st = self.lower_cst_expr(&expr_children[2]);
                        let b = self.lower_cst_expr(expr_children.last().unwrap());
                        (s, e, Some(st), b)
                    }
                };
                Expr::Foreach { pat, start, end, step, body }
            }
            SK::STRUCT_EXPR => {
                let children: Vec<_> = node.children().collect();
                let mut name = None;
                let mut fields = Vec::new();
                for child in &children {
                    if ast::IdentExpr::can_cast(child.kind()) || child.kind() == SK::IDENT {
                        name = self.first_ident_text(child);
                    } else if ast::FieldInit::can_cast(child.kind()) {
                        if let Some(f) = self.lower_cst_field_init(child) {
                            fields.push(f);
                        }
                    }
                }
                Expr::Struct { name, fields }
            }
            SK::UPDATE_EXPR => {
                let children: Vec<_> = node.children().collect();
                let base = children
                    .first()
                    .map(|c| self.lower_cst_expr(c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                let mut fields = Vec::new();
                for child in children.iter().skip(1) {
                    if ast::FieldInit::can_cast(child.kind()) {
                        if let Some(f) = self.lower_cst_field_init(child) {
                            fields.push(f);
                        }
                    }
                }
                Expr::Update { base, fields }
            }
            SK::CAST_EXPR => {
                let children: Vec<_> = node.children().collect();
                let expr = children
                    .first()
                    .map(|c| self.lower_cst_expr(c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                Expr::Cast { expr, ty_span: span }
            }
            SK::ASSIGN_EXPR => {
                let children: Vec<_> = node.children().collect();
                let target = children
                    .first()
                    .map(|c| self.lower_cst_expr(c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                let value = children
                    .get(1)
                    .map(|c| self.lower_cst_expr(c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                Expr::Assign { target, value }
            }
            SK::VAR_EXPR => {
                let children: Vec<_> = node.children().collect();
                let target = children
                    .first()
                    .map(|c| self.lower_cst_expr(c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                let value = children
                    .get(1)
                    .map(|c| self.lower_cst_expr(c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                let body = children
                    .get(2)
                    .map(|c| self.lower_cst_expr(c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                Expr::Var { target, value, body }
            }
            SK::CONFIG_EXPR => {
                // Extract dot-separated path segments from config expression.
                let segments: Vec<String> = node
                    .children_with_tokens()
                    .filter_map(|el| el.into_token())
                    .filter(|tok| tok.kind() == SK::IDENT)
                    .map(|tok| tok.text().to_string())
                    .collect();
                if segments.is_empty() {
                    // Fallback for legacy $[...] config syntax
                    let text = node.text().to_string().trim().to_string();
                    Expr::Config(vec![text])
                } else {
                    Expr::Config(segments)
                }
            }
            SK::ATTRIBUTE => {
                if let Some(child) = node.children().next() {
                    return self.lower_cst_expr(&child);
                }
                Expr::Attribute { expr: self.alloc_expr(Expr::Missing, span) }
            }
            _ => {
                if let Some(child) = node.children().next() {
                    return self.lower_cst_expr(&child);
                }
                let text = node.text().to_string().trim().to_string();
                if text.is_empty() {
                    Expr::Missing
                } else {
                    Expr::Ident(text)
                }
            }
        };
        self.alloc_expr_cst(hir, node)
    }

    fn lower_cst_field_init(&mut self, node: &syntax::SyntaxNode) -> Option<(String, ExprId)> {
        let name = self.first_ident_text(node)?;
        let value_child = node.children().next();
        let value = value_child.map(|c| self.lower_cst_expr(&c)).unwrap_or_else(|| {
            let span = Self::text_range_to_span(node.text_range());
            let hir = Expr::Ident(name.clone());
            self.alloc_expr(hir, span)
        });
        Some((name, value))
    }

    fn lower_cst_match_arm(
        &mut self,
        node: &syntax::SyntaxNode,
    ) -> crate::expr_store::hir::MatchArm {
        use parser::SyntaxKind as SK;
        let span = Self::text_range_to_span(node.text_range());

        let has_if = node
            .children_with_tokens()
            .any(|el| matches!(el, rowan::NodeOrToken::Token(ref t) if t.kind() == SK::KW_IF));

        let children: Vec<_> = node.children().collect();
        let pat = children
            .first()
            .map(|c| self.lower_cst_pat(c))
            .unwrap_or_else(|| self.alloc_pat(Pat::Missing, span));

        let (guard, body) = if has_if && children.len() >= 3 {
            let g = self.lower_cst_expr(&children[1]);
            let b = self.lower_cst_expr(&children[2]);
            (Some(g), b)
        } else {
            let b = children
                .last()
                .map(|c| self.lower_cst_expr(c))
                .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
            (None, b)
        };
        crate::expr_store::hir::MatchArm { pat, guard, body }
    }

    fn lower_cst_block_item_into(
        &mut self,
        node: &syntax::SyntaxNode,
        out: &mut Vec<crate::expr_store::hir::Statement>,
    ) {
        let span = Self::text_range_to_span(node.text_range());
        if let Some(child) = node.children().next() {
            use parser::SyntaxKind as SK;
            match child.kind() {
                SK::LET_EXPR => {
                    let children: Vec<_> = child.children().collect();
                    let pat = children
                        .first()
                        .map(|c| self.lower_cst_pat(c))
                        .unwrap_or_else(|| self.alloc_pat(Pat::Missing, span));
                    let value = children
                        .get(1)
                        .map(|c| self.lower_cst_expr(c))
                        .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                    out.push(crate::expr_store::hir::Statement::Let { pat, value });
                    if let Some(body_node) = children.get(2) {
                        let body_id = self.lower_cst_expr(body_node);
                        out.push(crate::expr_store::hir::Statement::Expr(body_id));
                    }
                }
                SK::VAR_EXPR => {
                    // Lower var binding as Statement::Var { pat, value },
                    // matching Statement::Let pattern. Target is lowered as
                    // Pat::Bind (not Expr::Ident).
                    let children: Vec<_> = child.children().collect();
                    let pat = children
                        .first()
                        .map(|c| self.lower_cst_pat(c))
                        .unwrap_or_else(|| self.alloc_pat(Pat::Missing, span));
                    let value = children
                        .get(1)
                        .map(|c| self.lower_cst_expr(c))
                        .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                    out.push(crate::expr_store::hir::Statement::Var { pat, value });
                    // If there's a body child (var x = v in body), push as stmt
                    if let Some(body_node) = children.get(2) {
                        let body_id = self.lower_cst_expr(body_node);
                        out.push(crate::expr_store::hir::Statement::Expr(body_id));
                    }
                }
                _ => {
                    let id = self.lower_cst_expr(&child);
                    out.push(crate::expr_store::hir::Statement::Expr(id));
                }
            }
        } else {
            let id = self.alloc_expr(Expr::Missing, span);
            out.push(crate::expr_store::hir::Statement::Expr(id));
        }
    }

    #[allow(dead_code)]
    fn lower_cst_block_item(
        &mut self,
        node: &syntax::SyntaxNode,
    ) -> crate::expr_store::hir::Statement {
        let span = Self::text_range_to_span(node.text_range());
        if let Some(child) = node.children().next() {
            use parser::SyntaxKind as SK;
            match child.kind() {
                SK::LET_EXPR => {
                    let children: Vec<_> = child.children().collect();
                    let pat = children
                        .first()
                        .map(|c| self.lower_cst_pat(c))
                        .unwrap_or_else(|| self.alloc_pat(Pat::Missing, span));
                    let value = children
                        .get(1)
                        .map(|c| self.lower_cst_expr(c))
                        .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                    crate::expr_store::hir::Statement::Let { pat, value }
                }
                _ => {
                    let id = self.lower_cst_expr(&child);
                    crate::expr_store::hir::Statement::Expr(id)
                }
            }
        } else {
            let id = self.alloc_expr(Expr::Missing, span);
            crate::expr_store::hir::Statement::Expr(id)
        }
    }

    /// Lower a CST pattern node.
    pub(crate) fn lower_cst_pat(&mut self, node: &syntax::SyntaxNode) -> PatId {
        use parser::SyntaxKind as SK;
        let span = Self::text_range_to_span(node.text_range());
        let hir = match node.kind() {
            SK::WILD_PAT => Pat::Wild,
            SK::LITERAL_PAT => {
                let text = node.text().to_string().trim().to_string();
                Pat::Literal(self.literal_from_text_to_lit(&text))
            }
            SK::IDENT_PAT | SK::IDENT_EXPR => {
                let name = self
                    .first_ident_text(node)
                    .unwrap_or_else(|| node.text().to_string().trim().to_string());
                Pat::Bind(name)
            }
            SK::TYVAR_PAT => {
                let text = node.text().to_string().trim().to_string();
                Pat::TypeVar(text)
            }
            SK::APP_PAT => {
                let children: Vec<_> = node.children().collect();
                let ctor = children
                    .first()
                    .and_then(|c| self.first_ident_text(c))
                    .or_else(|| self.first_ident_text(node))
                    .unwrap_or_default();
                let args: Vec<_> = children.iter().skip(1).map(|c| self.lower_cst_pat(c)).collect();
                Pat::App { ctor, args }
            }
            SK::TUPLE_PAT => {
                let items: Vec<_> = node.children().map(|c| self.lower_cst_pat(&c)).collect();
                Pat::Tuple(items)
            }
            SK::LIST_PAT => {
                let items: Vec<_> = node.children().map(|c| self.lower_cst_pat(&c)).collect();
                Pat::List(items)
            }
            SK::VECTOR_PAT => {
                let items: Vec<_> = node.children().map(|c| self.lower_cst_pat(&c)).collect();
                Pat::Array(items)
            }
            SK::TYPED_PAT => {
                let children: Vec<_> = node.children().collect();
                let inner = children
                    .first()
                    .map(|c| self.lower_cst_pat(c))
                    .unwrap_or_else(|| self.alloc_pat(Pat::Missing, span));
                let ty_span = children
                    .get(1)
                    .map(|c| Self::text_range_to_span(c.text_range()))
                    .unwrap_or(span);
                Pat::Typed { inner, ty_span }
            }
            SK::AS_PAT => {
                let children: Vec<_> = node.children().collect();
                let pat = children
                    .first()
                    .map(|c| self.lower_cst_pat(c))
                    .unwrap_or_else(|| self.alloc_pat(Pat::Missing, span));
                let binding = self.last_ident_text(node).unwrap_or_default();
                Pat::As { pat, binding }
            }
            SK::BIN_PAT => {
                let children: Vec<_> = node.children().collect();
                let lhs = children
                    .first()
                    .map(|c| self.lower_cst_pat(c))
                    .unwrap_or_else(|| self.alloc_pat(Pat::Missing, span));
                let op = self.infix_op_text(node);
                let rhs = children
                    .last()
                    .map(|c| self.lower_cst_pat(c))
                    .unwrap_or_else(|| self.alloc_pat(Pat::Missing, span));
                Pat::Infix { lhs, op, rhs }
            }
            SK::STRUCT_PAT => {
                let name = self.first_ident_text(node);
                let mut fields = Vec::new();
                let mut has_wildcard = false;
                for child in node.children() {
                    if ast::FieldInit::can_cast(child.kind()) {
                        if let Some(field_name) = self.first_ident_text(&child) {
                            let pat_child = child.children().next();
                            let pat =
                                pat_child.map(|c| self.lower_cst_pat(&c)).unwrap_or_else(|| {
                                    let bind = Pat::Bind(field_name.clone());
                                    self.alloc_pat(bind, span)
                                });
                            fields.push((field_name, pat));
                        } else {
                            // FIELD_INIT with no IDENT — check for underscore
                            // token (wildcard `_` meaning "ignore rest").
                            let is_underscore = child
                                .children_with_tokens()
                                .filter_map(|el| el.into_token())
                                .any(|t| t.text() == "_");
                            if is_underscore {
                                has_wildcard = true;
                            }
                        }
                    }
                }
                // Record `_` wildcard as a synthetic field so that the
                // missing-fields check in pat.rs recognises it.
                if has_wildcard {
                    let wild_pat = self.alloc_pat(Pat::Wild, span);
                    fields.push(("_".to_string(), wild_pat));
                }
                Pat::Struct { name, fields }
            }
            SK::INDEX_PAT | SK::RANGE_INDEX_PAT => {
                let children: Vec<_> = node.children().collect();
                let name = children
                    .first()
                    .and_then(|c| {
                        c.descendants_with_tokens()
                            .filter_map(|el| el.into_token())
                            .find(|t| t.kind() == SK::IDENT)
                            .map(|t| t.text().to_string())
                    })
                    .unwrap_or_default();
                if ast::RangeIndexPat::can_cast(node.kind()) {
                    let start_span = children
                        .get(1)
                        .map(|c| Self::text_range_to_span(c.text_range()))
                        .unwrap_or(span);
                    let end_span = children
                        .get(2)
                        .map(|c| Self::text_range_to_span(c.text_range()))
                        .unwrap_or(span);
                    Pat::RangeIndex { name, start_span, end_span }
                } else {
                    let index_span = children
                        .get(1)
                        .map(|c| Self::text_range_to_span(c.text_range()))
                        .unwrap_or(span);
                    Pat::Index { name, index_span }
                }
            }
            SK::ATTRIBUTE => {
                if let Some(child) = node.children().next() {
                    return self.lower_cst_pat(&child);
                }
                Pat::Missing
            }
            _ => {
                let text = node.text().to_string().trim().to_string();
                if text == "_" {
                    Pat::Wild
                } else if text.is_empty() {
                    Pat::Missing
                } else {
                    Pat::Bind(text)
                }
            }
        };
        self.alloc_pat_cst(hir, node)
    }

    fn first_ident_text(&self, node: &syntax::SyntaxNode) -> Option<String> {
        use parser::SyntaxKind as SK;
        node.children_with_tokens()
            .filter_map(|el| el.into_token())
            .find(|t| t.kind() == SK::IDENT)
            .map(|t| t.text().to_string())
    }

    fn last_ident_text(&self, node: &syntax::SyntaxNode) -> Option<String> {
        use parser::SyntaxKind as SK;
        node.children_with_tokens()
            .filter_map(|el| el.into_token())
            .filter(|t| t.kind() == SK::IDENT)
            .last()
            .map(|t| t.text().to_string())
    }

    fn first_non_trivia_token_text(&self, node: &syntax::SyntaxNode) -> String {
        node.children_with_tokens()
            .filter_map(|el| el.into_token())
            .find(|t| !t.kind().is_trivia())
            .map(|t| t.text().to_string())
            .unwrap_or_default()
    }

    fn infix_op_text(&self, node: &syntax::SyntaxNode) -> String {
        use parser::SyntaxKind as SK;
        node.children_with_tokens()
            .filter_map(|el| el.into_token())
            .find(|t| {
                let k = t.kind();
                !k.is_trivia()
                    && k != SK::IDENT
                    && k != SK::NUM_LIT
                    && k != SK::BIN_LIT
                    && k != SK::HEX_LIT
                    && k != SK::STRING_LIT
                    && k != SK::TY_VAR
                    && k != SK::KW_TRUE
                    && k != SK::KW_FALSE
            })
            .map(|t| t.text().to_string())
            .unwrap_or_else(|| "+".to_string())
    }

    fn literal_from_text(&self, text: &str) -> Expr {
        Expr::Literal(self.literal_from_text_to_lit(text))
    }

    fn literal_from_text_to_lit(&self, text: &str) -> parser::Literal {
        use parser::Literal;
        match text {
            "true" => Literal::Bool(true),
            "false" => Literal::Bool(false),
            "()" => Literal::Unit,
            "undefined" => Literal::Undefined,
            "bitzero" => Literal::BitZero,
            "bitone" => Literal::BitOne,
            s if s.starts_with("0b") => Literal::Binary(s[2..].to_string()),
            s if s.starts_with("0x") => Literal::Hex(s[2..].to_string()),
            s if s.starts_with('"') => Literal::String(s.trim_matches('"').to_string()),
            s if s.contains('.') => Literal::Number(s.to_string()),
            s => Literal::Number(s.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr_store::body::pat_id_from_raw;

    #[test]
    fn flat_expression_has_one_node() {
        let (body, map) = cst_body_with_map("function f() = 42\n");
        assert_eq!(body.len(), 1);
        assert_eq!(map.len(), 1);
        assert_eq!(u32::from(body.root().into_raw()), 0);
        assert!(matches!(body[body.root()], Expr::Literal(_)));
    }

    /// J3-6: undefined parses as Literal::Undefined in expression position.
    /// Upstream (commit 0fdec9e2) moved undefined from literal to expression;
    /// sail-lsp handles it as Literal::Undefined which works in all positions.
    #[test]
    fn undefined_lowers_to_literal() {
        let body = cst_body("function f() = undefined\n");
        assert_eq!(body.len(), 1);
        match &body[body.root()] {
            Expr::Literal(parser::Literal::Undefined) => {} // correct
            other => panic!("expected Literal(Undefined), got {:?}", other),
        }
    }

    #[test]
    fn binary_expression_has_three_nodes() {
        let body = cst_body("function f(x, y) = x + y\n");
        assert_eq!(body.len(), 3);
        assert!(matches!(body[body.root()], Expr::BinaryOp { .. }));
    }

    #[test]
    fn root_id_is_zero_indexed() {
        let body = cst_body("function f() = 1\n");
        assert_eq!(u32::from(body.root().into_raw()), 0);
    }

    #[test]
    fn body_source_map_round_trip() {
        let (body, map) = cst_body_with_map("function f(x, y) = x + y\n");
        for (id, _) in body.iter_exprs() {
            let span = map.expr_syntax(id).expect("span for expr");
            let resolved = map.expr_at_span(span).expect("span hit");
            assert_eq!(resolved, id);
        }
    }

    #[test]
    fn if_expression_lowers_correctly() {
        let body = cst_body("function f(x) = if x == 0 then 1 else 2\n");
        assert!(matches!(body[body.root()], Expr::If { .. }));
        assert_eq!(body.len(), 6);
    }

    #[test]
    fn match_expression_lowers_arms() {
        let body = cst_body("function f(x) = match x { 0 => 10, _ => 20 }\n");
        match &body[body.root()] {
            Expr::Match { arms, .. } => assert_eq!(arms.len(), 2),
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn call_expression_lowers_callee_and_args() {
        let body = cst_body("function f() = add(1, 2)\n");
        match &body[body.root()] {
            Expr::Call { callee, args } => {
                assert!(matches!(body[*callee], Expr::Ident(_)));
                assert_eq!(args.len(), 2);
            }
            other => panic!("expected Call, got {:?}", other),
        }
    }

    #[test]
    fn block_with_let_lowers_statements() {
        let body = cst_body("function f() = { let x = 1; x + 2 }\n");
        match &body[body.root()] {
            Expr::Block(stmts) => assert_eq!(stmts.len(), 2),
            other => panic!("expected Block, got {:?}", other),
        }
    }

    #[test]
    fn body_without_patterns_has_empty_pat_arena() {
        let (body, map) = cst_body_with_map("function f(x, y) = x + y\n");
        assert_eq!(body.pats_len(), 0);
        assert_eq!(map.pats_len(), 0);
    }

    #[test]
    fn let_binding_pattern_enters_pat_arena() {
        let (body, map) = cst_body_with_map("function f() = { let x = 1; x }\n");
        assert_eq!(body.pats_len(), 1);
        assert_eq!(map.pats_len(), 1);
        assert!(matches!(body.pat(pat_id_from_raw(0)), Some(Pat::Bind(_))));
    }

    #[test]
    fn match_arm_patterns_enter_pat_arena() {
        let (body, _map) = cst_body_with_map("function f(x) = match x { 0 => 100, _ => 200 }\n");
        assert_eq!(body.pats_len(), 2);
    }

    #[test]
    fn nested_constructor_pattern_enters_arena_recursively() {
        let source = "\
union opt('a) = { None : unit, Some : 'a }
function f(x : opt(int)) -> int = match x {
    None() => 0,
    Some(0) => 1,
    Some(_) => 2,
}
";
        let body = cst_body(source);
        assert!(body.pats_len() >= 5);
    }

    #[test]
    fn iter_exprs_visits_all() {
        let body = cst_body("function f(x, y) = x + y\n");
        let count = body.iter_exprs().count();
        assert_eq!(count, body.len());
    }

    #[test]
    fn expr_map_back_round_trip() {
        let (body, map) = cst_body_with_map("function f(x, y) = x + y\n");
        for (id, _) in body.iter_exprs() {
            let _back = map.expr_syntax(id).expect("expr_map_back should have entry");
        }
    }

    #[test]
    fn pat_map_back_round_trip() {
        let (body, map) = cst_body_with_map("function f() = { let x = 1; x }\n");
        for (id, _) in body.iter_pats() {
            let _back = map.pat_syntax(id).expect("pat_map_back should have entry");
        }
    }

    #[test]
    fn forward_reverse_consistency() {
        let (_, map) = cst_body_with_map("function f(x, y) = x + y\n");
        for (ptr, &id) in &map.store.expr_map {
            let back_ptr = map.store.expr_syntax_ptr(id).unwrap();
            assert_eq!(back_ptr.text_range(), ptr.text_range());
        }
    }

    #[test]
    fn cst_literal() {
        let body = cst_body("function f() = 42\n");
        assert!(body.len() >= 1);
        let has_lit = body.iter_exprs().any(|(_, e)| matches!(e, Expr::Literal(_)));
        assert!(has_lit);
    }

    #[test]
    fn cst_infix() {
        let body = cst_body("function f(x, y) = x + y\n");
        let has_infix = body.iter_exprs().any(|(_, e)| matches!(e, Expr::BinaryOp { .. }));
        assert!(has_infix);
    }

    #[test]
    fn cst_call() {
        let body = cst_body("function f() = add(1, 2)\n");
        let has_call = body.iter_exprs().any(|(_, e)| matches!(e, Expr::Call { .. }));
        assert!(has_call);
    }

    #[test]
    fn cst_if() {
        let body = cst_body("function f(x) = if x == 0 then 1 else 2\n");
        let has_if = body.iter_exprs().any(|(_, e)| matches!(e, Expr::If { .. }));
        assert!(has_if);
    }

    #[test]
    fn cst_match_with_patterns() {
        let body = cst_body("function f(x) = match x { 0 => 10, _ => 20 }\n");
        let has_match = body.iter_exprs().any(|(_, e)| matches!(e, Expr::Match { .. }));
        assert!(has_match);
        assert!(body.pats_len() >= 2);
    }

    #[test]
    fn cst_block() {
        let body = cst_body("function f() = { let x = 1; x + 2 }\n");
        let has_block = body.iter_exprs().any(|(_, e)| matches!(e, Expr::Block(_)));
        assert!(has_block);
    }

    fn cst_body_with_map(source: &str) -> (Body, BodySourceMap) {
        use syntax::ast::{AstNode as _, CallableDef};
        let (root, _) = syntax::parse_text(source);
        for def in root.children().filter_map(CallableDef::cast) {
            if let Some(body_node) =
                crate::bodies::CallableBodies::cst_callable_body_node(def.syntax())
            {
                return Body::lower_body(&body_node);
            }
        }
        Body::lower_body(&root)
    }

    fn cst_body(source: &str) -> Body {
        cst_body_with_map(source).0
    }
}
