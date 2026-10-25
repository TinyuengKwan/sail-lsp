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
    lower_body_with_params_at(root, param_list_node, 0)
}

/// As `lower_body_with_params`, with every span the parameters produce
/// shifted by `param_span_offset`.
pub fn lower_body_with_params_at(
    root: &syntax::SyntaxNode,
    param_list_node: Option<&syntax::SyntaxNode>,
    param_span_offset: i64,
) -> (Body, BodySourceMap) {
    use parser::SyntaxKind as SK;

    let mut ctx = ExprCollector::new();
    ctx.span_offset = param_span_offset;

    // `function f (A(v) if v == 0x0) = ..` carries its guard inside the
    // parameter list, as bare tokens after `if`. Nothing lowered it, so a
    // guard was never inferred and any type error inside one was invisible.
    let guard_src = param_list_node.and_then(guard_source_text);

    let params: Vec<PatId> = if let Some(pl) = param_list_node {
        // First try child NODES (structured patterns like IDENT_PAT, APP_PAT, etc.)
        // `()` is a single token, not a node, so a parameter list like
        // `(() : unit, i : int)` used to yield one parameter instead of two
        // and every later parameter was matched against the wrong type.
        // Whether the list joins its contents into larger patterns. A
        // concatenation parameter — `a : bits(4) @ b : bits(1) @ .. @
        // 0b0000110` — arrives as flat tokens, and its literal is a *part*
        // of one parameter, not a parameter. Only a plain comma-separated
        // list of atoms may have its literal tokens taken as parameters.
        let joined = pl
            .children_with_tokens()
            .any(|el| matches!(el.kind(), SK::AT | SK::COLON | SK::OP_IDENT));
        // Everything from a top-level `if` on is the guard, not a parameter.
        // Collecting it left `A(v)` unbound in `f (A(v) if v == 0x0)` — the
        // guard's tokens sit in the same list.
        let guard_start = {
            let mut depth: i32 = 0;
            let mut at = None;
            for el in pl.children_with_tokens() {
                match el.kind() {
                    SK::L_PAREN | SK::L_BRACK | SK::L_CURLY => depth += 1,
                    SK::R_PAREN | SK::R_BRACK | SK::R_CURLY => depth -= 1,
                    SK::KW_IF if depth == 1 && at.is_none() => {
                        at = Some(el.text_range().start());
                    }
                    _ => {}
                }
            }
            at
        };
        let before_guard = |el: &rowan::NodeOrToken<syntax::SyntaxNode, syntax::SyntaxToken>| {
            guard_start.is_none_or(|g| el.text_range().start() < g)
        };
        // `function g(Regno(n)) = ..` — a constructor application as a
        // parameter pattern. The list arrives as flat tokens, so the walk
        // below took `Regno` for one binding and `n` for another: a
        // one-parameter function came out with *two*, the first shadowing a
        // constructor name, and nothing checked the payload. The fallback
        // loop further down already recognised `IDENT (`; it never ran,
        // because this walk is only skipped when it produces nothing.
        let elements_all: Vec<_> = pl.children_with_tokens().filter(before_guard).collect();
        let mut ctor_apps: Vec<(rowan::TextRange, PatId)> = Vec::new();
        let mut covered: Vec<rowan::TextRange> = Vec::new();
        {
            use rowan::NodeOrToken;
            let mut i = 0usize;
            while i < elements_all.len() {
                let NodeOrToken::Token(tok) = &elements_all[i] else {
                    i += 1;
                    continue;
                };
                if tok.kind() != SK::IDENT
                    || elements_all.get(i + 1).map(|e| e.kind()) != Some(SK::L_PAREN)
                {
                    i += 1;
                    continue;
                }
                let mut depth = 0usize;
                let mut j = i + 1;
                let mut args = Vec::new();
                while j < elements_all.len() {
                    match elements_all[j].kind() {
                        SK::L_PAREN => depth += 1,
                        SK::R_PAREN => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        SK::IDENT if depth == 1 => {
                            if let NodeOrToken::Token(t) = &elements_all[j] {
                                let span = ExprCollector::text_range_to_span(t.text_range());
                                args.push(ctx.alloc_pat_bind_at(t.text().to_string(), span));
                            }
                        }
                        SK::UNDERSCORE if depth == 1 => {
                            if let NodeOrToken::Token(t) = &elements_all[j] {
                                let span = ExprCollector::text_range_to_span(t.text_range());
                                args.push(ctx.alloc_pat(Pat::Wild, span));
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                if j < elements_all.len() {
                    let at = rowan::TextRange::new(
                        tok.text_range().start(),
                        elements_all[j].text_range().end(),
                    );
                    let pat = ctx.alloc_pat(
                        Pat::App { ctor: tok.text().to_string(), args },
                        ExprCollector::text_range_to_span(at),
                    );
                    ctor_apps.push((at, pat));
                    covered.push(at);
                    i = j + 1;
                    continue;
                }
                i += 1;
            }
        }
        let inside_ctor =
            |r: rowan::TextRange| covered.iter().any(|c| c.contains_range(r) && *c != r);

        let mut from_nodes_at: Vec<(rowan::TextRange, PatId)> = pl
            .children_with_tokens()
            .filter(before_guard)
            .filter(|el| !inside_ctor(el.text_range()))
            .filter_map(|el| {
                let at = el.text_range();
                let pat = match el {
                    rowan::NodeOrToken::Node(c) => {
                        let k = c.kind();
                        (ast::Pat::can_cast(k) || ast::IdentExpr::can_cast(k))
                            .then(|| ctx.lower_cst_pat(&c))
                    }
                    // `()` and `_` are both single tokens, and both are a
                    // parameter. `_` was not collected, so
                    // `function clause is_CSR_accessible(0x304, _, _)` lowered
                    // to *one* parameter against three declared types — every
                    // clause with a wildcard parameter had the rest of its
                    // parameters unbound, across sail-riscv.
                    //
                    // `!joined` for the same reason as the literal and the
                    // bare binding below: a head that annotates or
                    // concatenates is read by the loop further down, and
                    // collecting one token here made this list non-empty and
                    // that loop unreachable — so `(() : unit, i : int)` bound
                    // nothing but the `()`.
                    rowan::NodeOrToken::Token(t)
                        if !joined && matches!(t.kind(), SK::UNIT | SK::UNDERSCORE) =>
                    {
                        let span = ExprCollector::text_range_to_span(t.text_range());
                        Some(ctx.alloc_pat(Pat::Wild, span))
                    }
                    // A literal parameter pattern is a bare token here, not a
                    // node: `function f (0x0) = 0` puts a `HEX_LIT` straight
                    // inside the `PARAM_LIST`. Both collection paths looked for
                    // nodes, so the parameter list came out empty and the
                    // literal had nothing to be checked against — every literal
                    // parameter in every function clause went unchecked.
                    rowan::NodeOrToken::Token(t)
                        if !joined
                            && matches!(
                                t.kind(),
                                SK::HEX_LIT | SK::BIN_LIT | SK::NUM_LIT | SK::STRING_LIT
                            ) =>
                    {
                        let span = ExprCollector::text_range_to_span(t.text_range());
                        let lit = ctx.literal_from_text_to_lit(t.text().trim());
                        Some(ctx.alloc_pat(Pat::Literal(lit), span))
                    }
                    // A binding is a bare token here too, when the head is
                    // parenthesised for a guard: `function clause write_CSR
                    // ((0x15D, value) if xlen == 32)` puts `value` straight in
                    // the `PARAM_LIST` where the unguarded form gives it a node.
                    // Without this the guarded clause lowered to one parameter
                    // against two declared types.
                    rowan::NodeOrToken::Token(t) if !joined && t.kind() == SK::IDENT => {
                        let span = ExprCollector::text_range_to_span(t.text_range());
                        Some(ctx.alloc_pat_bind_at(t.text().to_string(), span))
                    }
                    rowan::NodeOrToken::Token(_) => None,
                };
                pat.map(|p| (at, p))
            })
            .collect();

        // `function test _ : int as x = x` — an unparenthesised head, where
        // the grammar bumps `as` and the name straight into the flat
        // `PARAM_LIST` (`params.rs:117`) instead of building an `AS_PAT`.
        // Nothing read the `as`, so the loop above took `x` for a *second
        // parameter*: the head declared one binding too many, only the
        // first was matched against the one declared type, and the body's
        // `x` resolved to nothing. Upstream's rule is the one the `AS_PAT`
        // lowering follows — a plain identifier on the right of `as` binds
        // the value it is written beside (`initial_check.ml:1329`).
        {
            let mut els = pl
                .children_with_tokens()
                .filter(before_guard)
                .skip_while(|el| el.kind() != SK::KW_AS);
            let name = els.next().and_then(|_| {
                els.find(|el| el.kind() != SK::WHITESPACE && !el.kind().is_trivia())
                    .and_then(|el| el.into_token())
                    .filter(|t| t.kind() == SK::IDENT)
            });
            if let Some(name) = name {
                let at = name.text_range();
                // The name may or may not have produced a pattern of its
                // own above — a joined head skips its bare tokens.
                from_nodes_at.retain(|(r, _)| *r != at);
                if let Some(last) = from_nodes_at.last_mut() {
                    if last.0.end() <= at.start() {
                        let span = ExprCollector::text_range_to_span(at);
                        last.1 = ctx.alloc_pat(
                            Pat::As { pat: last.1, binding: name.text().to_string() },
                            span,
                        );
                    }
                }
            }
        }
        // Only when this walk produces something anyway. An empty list hands
        // over to the fallback loop below, which already reads `IDENT (` —
        // and reads `IDENT : TYPE` too, which this pre-pass does not. A
        // comma-separated head takes that route (`joined` stops the walk
        // from collecting its bare tokens), and splicing a constructor in
        // here would make the list non-empty and skip it.
        if !from_nodes_at.is_empty() {
            from_nodes_at.extend(ctor_apps);
            from_nodes_at.sort_by_key(|(r, _)| r.start());
        }
        let from_nodes: Vec<PatId> = from_nodes_at.into_iter().map(|(_, p)| p).collect();

        // A `@` beside top-level commas: each comma-separated group is one
        // parameter, and a group containing `@` is one concatenation.
        //
        // `function clause is_CSR_accessible((0b0011001 @ index : bits(5),
        // _, _) if ..)` came out as two parameters against three declared
        // types: the `@` group contributed none, because a joined list
        // skips its bare tokens, and the branch below only fires when the
        // list has no comma and no guard.
        // At what nesting the parameter separators sit. A guard wraps the
        // whole head in a *second* pair of parentheses —
        // `((0b0011001 @ index : bits(5), _, _) if unsigned(index) >= 3)` —
        // so its commas are one level deeper than in an unguarded head, and
        // looking for them at depth 1 found none.
        let param_depth: i32 = {
            let opens = pl
                .children_with_tokens()
                .filter(|el| !el.kind().is_trivia())
                .take(2)
                .filter(|el| el.kind() == SK::L_PAREN)
                .count();
            if guard_start.is_some() && opens == 2 {
                2
            } else {
                1
            }
        };
        let concat_with_commas = {
            let mut depth: i32 = 0;
            let (mut at, mut comma) = (false, false);
            for el in pl.children_with_tokens() {
                if guard_start.is_some_and(|g| el.text_range().start() >= g) {
                    break;
                }
                match el.kind() {
                    SK::L_PAREN | SK::L_BRACK | SK::L_CURLY => depth += 1,
                    SK::R_PAREN | SK::R_BRACK | SK::R_CURLY => depth -= 1,
                    SK::AT if depth == param_depth => at = true,
                    SK::COMMA if depth == param_depth => comma = true,
                    _ => {}
                }
            }
            at && comma
        };
        if concat_with_commas {
            let els: Vec<_> = pl
                .children_with_tokens()
                .filter(|el| guard_start.is_none_or(|g| el.text_range().start() < g))
                .filter(|el| !el.kind().is_trivia())
                .collect();
            // Split on the commas that separate parameters, dropping the
            // list's own parentheses.
            let mut groups: Vec<Vec<usize>> = vec![Vec::new()];
            let mut depth: i32 = 0;
            for (i, el) in els.iter().enumerate() {
                match el.kind() {
                    SK::L_PAREN | SK::L_BRACK | SK::L_CURLY => {
                        depth += 1;
                        if depth > param_depth {
                            groups.last_mut().unwrap().push(i);
                        }
                    }
                    SK::R_PAREN | SK::R_BRACK | SK::R_CURLY => {
                        depth -= 1;
                        if depth >= param_depth {
                            groups.last_mut().unwrap().push(i);
                        }
                    }
                    SK::COMMA if depth == param_depth => groups.push(Vec::new()),
                    _ => groups.last_mut().unwrap().push(i),
                }
            }
            let mut params: Vec<PatId> = Vec::new();
            for group in groups.iter().filter(|g| !g.is_empty()) {
                // `x : T` binds `x`; `@` chains its parts; anything else is
                // one pattern, or a wildcard when it is only punctuation.
                let mut parts: Vec<PatId> = Vec::new();
                let mut run: Vec<usize> = Vec::new();
                let emit = |run: &mut Vec<usize>,
                            parts: &mut Vec<PatId>,
                            ctx: &mut ExprCollector| {
                    let idx: Vec<usize> = std::mem::take(run);
                    match idx.as_slice() {
                        // `x : T`, where `T` is however many tokens it takes.
                        // A parameter list is raw tokens at this point:
                        // `bits(5)` is four of them, not one, so requiring
                        // exactly three dropped the binding on the floor and
                        // `index` in `(0b1100000 @ index : bits(5), _) if ..`
                        // was bound by nothing.
                        [a, c, ty_idx @ ..]
                            if !ty_idx.is_empty()
                                && els[*a].kind() == SK::IDENT
                                && els[*c].kind() == SK::COLON =>
                        {
                            let name = els[*a].as_token().unwrap().text().to_string();
                            let span = ExprCollector::text_range_to_span(els[*a].text_range());
                            let inner = ctx.alloc_pat_bind_at(name, span);
                            let ty: String = ty_idx.iter().map(|&i| els[i].to_string()).collect();
                            parts.push(ctx.alloc_pat(Pat::Typed { inner, ty }, span));
                        }
                        named
                            if vector_pat_from_flat(&els, named, ctx)
                                .map(|p| parts.push(p))
                                .is_some() => {}
                        [only] => {
                            let span = ExprCollector::text_range_to_span(els[*only].text_range());
                            match &els[*only] {
                                rowan::NodeOrToken::Node(c) => parts.push(ctx.lower_cst_pat(c)),
                                rowan::NodeOrToken::Token(t) => match t.kind() {
                                    SK::IDENT => parts
                                        .push(ctx.alloc_pat_bind_at(t.text().to_string(), span)),
                                    SK::HEX_LIT | SK::BIN_LIT | SK::NUM_LIT | SK::STRING_LIT => {
                                        let lit = ctx.literal_from_text_to_lit(t.text().trim());
                                        parts.push(ctx.alloc_pat(Pat::Literal(lit), span));
                                    }
                                    _ => parts.push(ctx.alloc_pat(Pat::Wild, span)),
                                },
                            }
                        }
                        [] => {}
                        rest => {
                            let span = ExprCollector::text_range_to_span(els[rest[0]].text_range());
                            parts.push(ctx.alloc_pat(Pat::Wild, span));
                        }
                    }
                };
                for &i in group {
                    if els[i].kind() == SK::AT {
                        emit(&mut run, &mut parts, &mut ctx);
                    } else {
                        run.push(i);
                    }
                }
                emit(&mut run, &mut parts, &mut ctx);
                let span = ExprCollector::text_range_to_span(els[group[0]].text_range());
                let Some(&first) = parts.first() else { continue };
                let mut acc = first;
                for &next in &parts[1..] {
                    acc = ctx
                        .alloc_pat(Pat::Infix { lhs: acc, op: "@".to_string(), rhs: next }, span);
                }
                params.push(acc);
            }
            if !params.is_empty() {
                return finish_body_with_guard(ctx, params, root, guard_src);
            }
        }

        // A top-level `@` makes the whole list *one* concatenation
        // parameter. The node pass returns its pieces separately —
        // `decode(a : bits(5) @ b : bits(1) @ 0b0000011111)` came out as two
        // parameters against one declared type — which loses the `@` and
        // with it any way to check the widths add up.
        let concat_at_top = guard_start.is_none() && {
            let mut depth: i32 = 0;
            let mut found = false;
            let mut ok = true;
            for el in pl.children_with_tokens() {
                match el.kind() {
                    SK::L_PAREN | SK::L_BRACK | SK::L_CURLY => depth += 1,
                    SK::R_PAREN | SK::R_BRACK | SK::R_CURLY => depth -= 1,
                    SK::AT if depth == 1 => found = true,
                    SK::COMMA if depth == 1 => ok = false,
                    _ => {}
                }
            }
            ok && found
        };
        if concat_at_top {
            // The list is flat: `IDENT COLON TYPE_APP AT IDENT COLON TYPE_APP
            // AT BIN_LIT`. Each `@`-separated run is one part, and a run of
            // `IDENT : TYPE` is a typed binding.
            let els: Vec<_> = pl.children_with_tokens().collect();
            let mut parts: Vec<PatId> = Vec::new();
            let mut run: Vec<usize> = Vec::new();
            let flush = |run: &mut Vec<usize>, parts: &mut Vec<PatId>, ctx: &mut ExprCollector| {
                let idx: Vec<usize> = std::mem::take(run);
                let mut named: Vec<usize> =
                    idx.iter().copied().filter(|&i| !els[i].kind().is_trivia()).collect();
                // One balanced pair, if the part is parenthesised. Skipping
                // *every* paren — which is what happened — turns
                // `(c : bits(6))` into four elements rather than a name, a
                // colon and a type, so the arm below did not match and the
                // whole part was dropped: `c` unresolved and the widths short.
                while named.len() >= 2
                    && els[named[0]].kind() == SK::L_PAREN
                    && els[named[named.len() - 1]].kind() == SK::R_PAREN
                {
                    named = named[1..named.len() - 1].to_vec();
                }
                match named.as_slice() {
                    // `x : T`, where `T` may itself be an application.
                    [a, _colon, rest @ ..]
                        if !rest.is_empty()
                            && els[*a].kind() == SK::IDENT
                            && els[*_colon].kind() == SK::COLON =>
                    {
                        let name = els[*a].as_token().unwrap().text().to_string();
                        let span = ExprCollector::text_range_to_span(els[*a].text_range());
                        let inner = ctx.alloc_pat_bind_at(name, span);
                        let ty: String = rest.iter().map(|&i| els[i].to_string()).collect();
                        parts.push(ctx.alloc_pat(Pat::Typed { inner, ty }, span));
                    }
                    named
                        if vector_pat_from_flat(&els, named, ctx)
                            .map(|p| parts.push(p))
                            .is_some() => {}
                    [only] => match &els[*only] {
                        rowan::NodeOrToken::Node(c) => parts.push(ctx.lower_cst_pat(c)),
                        rowan::NodeOrToken::Token(t) => {
                            let span = ExprCollector::text_range_to_span(t.text_range());
                            match t.kind() {
                                SK::HEX_LIT | SK::BIN_LIT | SK::NUM_LIT => {
                                    let lit = ctx.literal_from_text_to_lit(t.text().trim());
                                    parts.push(ctx.alloc_pat(Pat::Literal(lit), span));
                                }
                                SK::IDENT => {
                                    parts.push(ctx.alloc_pat_bind_at(t.text().to_string(), span));
                                }
                                SK::UNDERSCORE => parts.push(ctx.alloc_pat(Pat::Wild, span)),
                                _ => {}
                            }
                        }
                    },
                    [] => {}
                    _ => {}
                }
            };
            // Every element but the list's own outer parentheses; `flush`
            // strips a part's own pair, and the parentheses *inside* a type
            // stay where they belong.
            let mut depth: i32 = 0;
            for (i, el) in els.iter().enumerate() {
                match el.kind() {
                    SK::L_PAREN | SK::L_BRACK | SK::L_CURLY => {
                        depth += 1;
                        if depth > 1 {
                            run.push(i);
                        }
                    }
                    SK::R_PAREN | SK::R_BRACK | SK::R_CURLY => {
                        depth -= 1;
                        if depth > 0 {
                            run.push(i);
                        }
                    }
                    SK::AT if depth == 1 => flush(&mut run, &mut parts, &mut ctx),
                    _ => run.push(i),
                }
            }
            flush(&mut run, &mut parts, &mut ctx);
            if parts.len() >= 2 {
                // The chain spans the whole list, minus its parentheses —
                // a zero span puts the diagnostic at the top of the file.
                let r = pl.text_range();
                let span = Span::new(usize::from(r.start()) + 1, usize::from(r.end()) - 1);
                let mut acc = parts[0];
                for &next in &parts[1..] {
                    acc = ctx
                        .alloc_pat(Pat::Infix { lhs: acc, op: "@".to_string(), rhs: next }, span);
                }
                return finish_body_with_guard(ctx, vec![acc], root, guard_src);
            }
        }
        if !from_nodes.is_empty() && !joined {
            from_nodes
        } else {
            // Fallback: extract params from raw tokens/nodes in PARAM_LIST.
            // PARAM_LIST contains tokens (IDENT, COMMA, COLON, parens) and
            // TYPE_* nodes from parsed type annotations.
            // Pattern: IDENT [COLON TYPE_*] → Pat::Bind or Pat::Typed
            use rowan::NodeOrToken;
            // The guard's tokens are in this list too; they are not
            // parameters.
            let elements: Vec<_> = pl.children_with_tokens().filter(before_guard).collect();
            // `f(h :: t)` is one cons pattern, not two parameters. The
            // parameter list arrives as flat tokens, so a top-level `::`
            // has to be recognised here or the head binds to the whole
            // list type.
            if let Some(cons) = lower_flat_cons_param(&mut ctx, &elements) {
                return finish_body_with_guard(ctx, vec![cons], root, guard_src);
            }
            let mut params = Vec::new();
            let mut i = 0;
            while i < elements.len() {
                // `A(v)` is one constructor pattern, not `A` and `v`. The
                // list arrives as flat tokens, so `IDENT (` has to be
                // recognised here — otherwise the payload binds to nothing
                // and a guard mentioning it has no type to check against.
                if let NodeOrToken::Token(tok) = &elements[i] {
                    if tok.kind() == SK::IDENT
                        && elements.get(i + 1).is_some_and(|e| e.kind() == SK::L_PAREN)
                    {
                        let ctor = tok.text().to_string();
                        let mut j = i + 2;
                        let mut args = Vec::new();
                        while j < elements.len() && elements[j].kind() != SK::R_PAREN {
                            match &elements[j] {
                                NodeOrToken::Token(t) if t.kind() == SK::IDENT => {
                                    let span = ExprCollector::text_range_to_span(t.text_range());
                                    args.push(ctx.alloc_pat_bind_at(t.text().to_string(), span));
                                }
                                NodeOrToken::Token(t) if t.kind() == SK::UNDERSCORE => {
                                    let span = ExprCollector::text_range_to_span(t.text_range());
                                    args.push(ctx.alloc_pat(Pat::Wild, span));
                                }
                                _ => {}
                            }
                            j += 1;
                        }
                        params.push(ctx.alloc_pat(
                            Pat::App { ctor, args },
                            ExprCollector::text_range_to_span(tok.text_range()),
                        ));
                        i = j + 1;
                        continue;
                    }
                }
                match &elements[i] {
                    NodeOrToken::Token(tok) => {
                        let k = tok.kind();
                        // `function a 'n = ..` — `atomic_pat: | kid` binds the
                        // value `n` and constrains it to `atom('n)`
                        // (parser.mly:541-543).
                        if k == SK::TY_VAR {
                            let name = tok.text().to_string();
                            params.push(ctx.alloc_pat(
                                crate::expr_store::hir::Pat::TypeVar(name),
                                Span {
                                    start: usize::from(tok.text_range().start()),
                                    end: usize::from(tok.text_range().end()),
                                },
                            ));
                            i += 1;
                            continue;
                        }
                        // `_ : int as x` — the binding on the right of `as`.
                        // The walk above owns this shape only while it
                        // collects the head's bare tokens; a head that
                        // annotates goes through here instead, and `x` was
                        // bound by nothing.
                        if k == SK::KW_AS {
                            let mut j = i + 1;
                            while j < elements.len() && elements[j].kind() == SK::WHITESPACE {
                                j += 1;
                            }
                            if let Some(NodeOrToken::Token(name)) = elements.get(j) {
                                if name.kind() == SK::IDENT {
                                    if let Some(inner) = params.pop() {
                                        let span =
                                            ExprCollector::text_range_to_span(name.text_range());
                                        params.push(ctx.alloc_pat(
                                            Pat::As {
                                                pat: inner,
                                                binding: name.text().to_string(),
                                            },
                                            span,
                                        ));
                                        i = j + 1;
                                        continue;
                                    }
                                }
                            }
                        }
                        // `()` is a parameter too, and it can be annotated:
                        // `function test(() : unit, i : int)`.
                        if k == SK::IDENT || k == SK::UNDERSCORE || k == SK::UNIT {
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
                                                    let name_span = Span {
                                                        start: usize::from(
                                                            tok.text_range().start(),
                                                        ),
                                                        end: usize::from(tok.text_range().end()),
                                                    };
                                                    let inner_pat =
                                                        ctx.alloc_pat_bind_at(name, name_span);
                                                    let typed = Pat::Typed {
                                                        inner: inner_pat,
                                                        ty: type_node.text().to_string(),
                                                    };
                                                    // The whole `x : T`, so a
                                                    // feature that wants the
                                                    // parameter gets it and one
                                                    // that wants the name goes
                                                    // through `inner`.
                                                    params.push(ctx.alloc_pat(
                                                        typed,
                                                        Span {
                                                            start: name_span.start,
                                                            end: usize::from(
                                                                type_node.text_range().end(),
                                                            ),
                                                        },
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
                            let span = ExprCollector::text_range_to_span(tok.text_range());
                            params.push(ctx.alloc_pat_bind_at(name, span));
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

    finish_body_with_guard(ctx, params, root, guard_src)
}

/// `[b]`, `[b, c]` — a vector pattern written inline in a parameter list.
///
/// A parameter list is raw tokens at this point, so this arrives as
/// `L_BRACK IDENT R_BRACK`. Both flatteners had an arm for one token and
/// one for `x : T`, and dropped everything else on the floor. In a
/// concatenation that lost the binding *and* the part's width, so
/// `f(0b01 @ [b] @ 0b0)` summed to 3 against a `bits(4)` and every use of
/// `b` was reported unresolved — which a text scan for "this name looks
/// bound somewhere" then hid.
fn vector_pat_from_flat(
    els: &[rowan::NodeOrToken<syntax::SyntaxNode, syntax::SyntaxToken>],
    named: &[usize],
    ctx: &mut ExprCollector,
) -> Option<PatId> {
    use parser::SyntaxKind as SK;
    let (&open, rest) = named.split_first()?;
    let (&close, inner) = rest.split_last()?;
    if els[open].kind() != SK::L_BRACK || els[close].kind() != SK::R_BRACK {
        return None;
    }
    let items: Vec<PatId> = inner
        .iter()
        .filter(|&&i| els[i].kind() != SK::COMMA)
        .map(|&i| {
            let span = ExprCollector::text_range_to_span(els[i].text_range());
            match &els[i] {
                rowan::NodeOrToken::Token(t) if t.kind() == SK::IDENT => {
                    ctx.alloc_pat_bind_at(t.text().to_string(), span)
                }
                rowan::NodeOrToken::Node(c) => ctx.lower_cst_pat(c),
                _ => ctx.alloc_pat(Pat::Wild, span),
            }
        })
        .collect();
    let span = ExprCollector::text_range_to_span(els[open].text_range());
    Some(ctx.alloc_pat(Pat::Array(items), span))
}

/// Assemble a `Body` from a collector, its parameters, and the body node.
fn finish_body_with_guard(
    mut ctx: ExprCollector,
    params: Vec<PatId>,
    root: &syntax::SyntaxNode,
    guard_src: Option<(String, usize)>,
) -> (Body, BodySourceMap) {
    let body_expr = ctx.lower_cst_expr(root);
    // The guard is parsed on its own — it was never a node, so there is
    // nothing in this tree to lower.
    ctx.span_offset = 0;

    // Parsed inside a function body: `parse_text` reads a whole file, and a
    // bare `v == 0x0` is not a definition.
    let guard = guard_src.and_then(|(text, abs_start)| {
        let prefix = "function __guard() = { ";
        let wrapped = format!("{prefix}{text} }}\n");
        let (groot, errors) = syntax::parse_text(&wrapped);
        if !errors.is_empty() {
            return None;
        }
        // The first expression inside the synthesised body. `BLOCK_EXPR`'s
        // own children are the braces and statements, so the search is over
        // descendants.
        let block = groot.descendants().find(|n| n.kind() == parser::SyntaxKind::BLOCK_EXPR)?;
        let expr =
            block.descendants().find(|n| n != &block && syntax::ast::Expr::can_cast(n.kind()))?;
        // Spans recorded from here point into `wrapped`; shift them back
        // onto the file the guard was written in.
        ctx.span_offset = abs_start as i64 - prefix.len() as i64;
        let id = ctx.lower_cst_expr(&expr);
        ctx.span_offset = 0;
        Some(id)
    });
    let mapping_arms = std::mem::take(&mut ctx.mapping_arms);
    let (store, store_source_map) = ctx.builder.finish();

    let body = Body { store, params: params.into_boxed_slice(), body_expr, mapping_arms, guard };
    let source_map = BodySourceMap { file_id: None, store: store_source_map, def_base: 0 };
    (body, source_map)
}

/// Recognise a flat `h :: t` parameter list as a single cons pattern.
/// The source text of a function clause's guard, if it has one.
///
/// The guard is everything after a top-level `if` in the parameter list,
/// minus the closing paren. Taken as text because the tokens were never
/// grouped into a node.
fn guard_source_text(pl: &syntax::SyntaxNode) -> Option<(String, usize)> {
    use parser::SyntaxKind as SK;
    let mut depth: i32 = 0;
    let mut start: Option<usize> = None;
    let mut end = 0usize;
    for el in pl.children_with_tokens() {
        let r = el.text_range();
        match el.kind() {
            SK::L_PAREN | SK::L_BRACK | SK::L_CURLY => depth += 1,
            SK::R_PAREN | SK::R_BRACK | SK::R_CURLY => {
                if depth == 1 && start.is_some() {
                    break;
                }
                depth -= 1;
            }
            SK::KW_IF if depth == 1 && start.is_none() => {
                start = Some(usize::from(r.end()));
            }
            _ => {}
        }
        end = usize::from(r.end());
    }
    let start = start?;
    let text = pl.text().to_string();
    let base = usize::from(pl.text_range().start());
    let (from, to) = (start.checked_sub(base)?, end.checked_sub(base)?);
    let raw = text.get(from..to.min(text.len()))?;
    let trimmed = raw.trim_start();
    let lead = raw.len() - trimmed.len();
    let slice = trimmed.trim_end().trim_end_matches(')').trim_end();
    (!slice.is_empty()).then(|| (slice.to_string(), start + lead))
}

fn lower_flat_cons_param(
    ctx: &mut ExprCollector,
    elements: &[rowan::NodeOrToken<syntax::SyntaxNode, syntax::SyntaxToken>],
) -> Option<PatId> {
    use parser::SyntaxKind as SK;
    use rowan::NodeOrToken;

    let idents: Vec<String> = elements
        .iter()
        .filter_map(|el| el.as_token())
        .filter(|t| t.kind() == SK::IDENT)
        .map(|t| t.text().to_string())
        .collect();
    let scopes =
        elements.iter().filter_map(|el| el.as_token()).filter(|t| t.kind() == SK::SCOPE).count();
    // Exactly `h :: t`, with nothing else in the list.
    if scopes != 1 || idents.len() != 2 {
        return None;
    }
    if elements.iter().any(|el| matches!(el, NodeOrToken::Node(_))) {
        return None;
    }
    if elements
        .iter()
        .filter_map(|el| el.as_token())
        .any(|t| matches!(t.kind(), SK::COMMA | SK::COLON))
    {
        return None;
    }
    let ident_spans: Vec<Span> = elements
        .iter()
        .filter_map(|el| el.as_token())
        .filter(|t| t.kind() == SK::IDENT)
        .map(|t| ExprCollector::text_range_to_span(t.text_range()))
        .collect();
    let span_of = |i: usize| ident_spans.get(i).copied().unwrap_or(Span { start: 0, end: 0 });
    let lhs = ctx.alloc_pat_bind_at(idents[0].clone(), span_of(0));
    let rhs = ctx.alloc_pat_bind_at(idents[1].clone(), span_of(1));
    Some(ctx.alloc_pat(
        crate::hir::Pat::Infix { lhs, op: "::".to_string(), rhs },
        Span { start: 0, end: 0 },
    ))
}

/// Accumulates HIR expressions and patterns during CST lowering.
pub(crate) struct ExprCollector {
    pub(crate) builder: ExpressionStoreBuilder,
    /// Added to every span this collector records.
    ///
    /// A function clause's guard is parsed from a synthesised snippet — it
    /// was never a node in the file's own tree — so its ranges start from
    /// that snippet. Without the shift, a diagnostic inside a guard lands
    /// wherever the snippet's offsets happen to point.
    span_offset: i64,
    /// Mapping clauses seen while lowering, in source order.
    ///
    /// They are not expressions and do not belong in the block they are
    /// written in — `Body::mapping_arms` is their home.
    mapping_arms: Vec<crate::expr_store::hir::MappingArm>,
}

impl ExprCollector {
    pub(crate) fn new() -> Self {
        Self { builder: ExpressionStoreBuilder::new(), span_offset: 0, mapping_arms: Vec::new() }
    }

    /// Lower one `MAPPING_CLAUSE` into `self.mapping_arms`.
    ///
    /// `forwards P [if g] => E` and `backwards ..` bind on the head and use
    /// those bindings in the body; `P <-> Q` is both at once, so each side
    /// is recorded as a pattern *and* as an expression and the direction
    /// says which is read.
    fn lower_cst_mapping_clause(&mut self, node: &syntax::SyntaxNode) {
        use crate::expr_store::hir::{MappingArm, MappingDirection};
        use parser::SyntaxKind as SK;

        let dir = node
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .find_map(|t| match t.kind() {
                SK::KW_FORWARDS => Some(MappingDirection::Forwards),
                SK::KW_BACKWARDS => Some(MappingDirection::Backwards),
                _ => None,
            })
            .unwrap_or(MappingDirection::Bidirectional);
        let bidi = node
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|t| t.kind() == SK::DOUBLE_ARROW);

        let children: Vec<_> = node.children().collect();
        let span = Self::text_range_to_span(node.text_range());
        let mut it = children.into_iter();
        let Some(head) = it.next() else { return };
        let lhs_pat = Some(self.lower_cst_pat(&head));

        if bidi {
            // Which child is which is decided by the `<->`, not by
            // position: a guard is written `if g` *before* it —
            // `Fregidx(i) if get_config_use_abi_names() <-> ..` — or `when
            // g` *after* it, and counting children mistook one for the
            // other side.
            let arrow = node
                .children_with_tokens()
                .filter_map(|el| el.into_token())
                .find(|t| t.kind() == SK::DOUBLE_ARROW)
                .map(|t| t.text_range().start());
            let Some(arrow) = arrow else { return };
            let rest: Vec<_> = it.collect();
            let guard = rest
                .iter()
                .find(|c| c.text_range().start() < arrow)
                .or_else(|| rest.iter().filter(|c| c.text_range().start() > arrow).nth(1))
                .cloned();
            let Some(tail) = rest.iter().find(|c| c.text_range().start() > arrow).cloned() else {
                return;
            };
            let guard = guard.map(|g| self.lower_cst_expr(&g));
            let rhs_pat_id = self.lower_cst_pat(&tail);
            // Both sides of `<->` are read as a pattern *and* written as an
            // expression. Lowering the pattern node a second time through
            // `lower_cst_expr` does not work — it does not know pattern
            // kinds, and turned `0b00001` into an identifier — so the
            // expression is built from the pattern that was just lowered.
            let Some(head_pat) = lhs_pat else { return };
            let lhs_expr = self.pat_as_expr(head_pat, span);
            let rhs_expr = self.pat_as_expr(rhs_pat_id, span);
            let rhs_pat = Some(rhs_pat_id);
            self.mapping_arms.push(MappingArm {
                direction: MappingDirection::Bidirectional,
                lhs_pat,
                rhs_pat,
                lhs_expr,
                rhs_expr,
                guard,
                span,
                mirrored: true,
            });
            return;
        }

        // A guard sits between the head and the body, and only a guard can.
        let rest: Vec<_> = it.collect();
        let (guard, body) = match rest.len() {
            0 => return,
            1 => (None, rest.into_iter().next()),
            _ => {
                let mut r = rest.into_iter();
                let g = r.next();
                (g, r.next())
            }
        };
        let guard = guard.map(|g| self.lower_cst_expr(&g));
        let Some(body) = body else { return };
        let body_expr = self.lower_cst_expr(&body);
        // The head is the input side; the body is the output side. Which of
        // the two the mapping's declared input type applies to is what
        // `direction` records, and inference reads it from there.
        let (lhs_expr, rhs_expr) = (body_expr, body_expr);
        self.mapping_arms.push(MappingArm {
            direction: dir,
            lhs_pat,
            rhs_pat: None,
            lhs_expr,
            rhs_expr,
            guard,
            span,
            mirrored: false,
        });
    }

    /// The expression a mapping pattern also is.
    ///
    /// `span` is the fallback; where the pattern was lowered from a node of
    /// its own, that node's range is used, so a diagnostic on the right of
    /// `<->` underlines the right of `<->` and not the whole clause.
    fn pat_as_expr(&mut self, pat: PatId, span: Span) -> ExprId {
        let span = self
            .builder
            .pat_map_back
            .get(pat)
            .map(|ptr| Self::text_range_to_span(ptr.text_range()))
            .unwrap_or(span);
        let hir = match self.builder.pats[pat].clone() {
            Pat::Bind(name) => Expr::Ident(name),
            Pat::Literal(lit) => Expr::Literal(lit),
            Pat::App { ctor, args } => {
                let callee = self.alloc_expr(Expr::Ident(ctor), span);
                let args = args.iter().map(|&a| self.pat_as_expr(a, span)).collect();
                Expr::Call { callee, args }
            }
            Pat::Tuple(items) => {
                Expr::Tuple(items.iter().map(|&p| self.pat_as_expr(p, span)).collect())
            }
            Pat::List(items) => {
                Expr::List(items.iter().map(|&p| self.pat_as_expr(p, span)).collect())
            }
            Pat::Array(items) => {
                Expr::Array(items.iter().map(|&p| self.pat_as_expr(p, span)).collect())
            }
            Pat::Typed { inner, .. } | Pat::AsType { pat: inner, .. } => {
                return self.pat_as_expr(inner, span);
            }
            // `i : bits(1) @ o : bits(1) <-> bit_maybe_i(i) ^ bit_maybe_o(o)`
            // — the right side is a `^` chain, and dropping it lost every
            // use of what the left side bound.
            Pat::Infix { lhs, op, rhs } => {
                let lhs = self.pat_as_expr(lhs, span);
                let rhs = self.pat_as_expr(rhs, span);
                Expr::BinaryOp { lhs, op: crate::expr_store::hir::HirBinaryOp::from_str(&op), rhs }
            }
            Pat::As { pat: inner, .. } => return self.pat_as_expr(inner, span),
            _ => Expr::Missing,
        };
        self.alloc_expr(hir, span)
    }

    fn alloc_expr(&mut self, hir: Expr, span: Span) -> ExprId {
        let span = self.shift(span);
        self.builder.alloc_expr(hir, span)
    }

    fn alloc_expr_cst(&mut self, hir: Expr, node: &syntax::SyntaxNode) -> ExprId {
        if self.span_offset != 0 {
            let span = self.shift(Self::text_range_to_span(node.text_range()));
            return self.builder.alloc_expr(hir, span);
        }
        self.builder.alloc_expr_cst(hir, node)
    }

    pub(crate) fn alloc_pat(&mut self, hir: Pat, span: Span) -> PatId {
        let span = self.shift(span);
        let name = Self::binding_name_of_pat(&hir);
        let pat_id = self.builder.alloc_pat(hir, span);
        if let Some(n) = name {
            self.builder.alloc_binding(n, pat_id);
        }
        pat_id
    }

    fn alloc_pat_cst(&mut self, hir: Pat, node: &syntax::SyntaxNode) -> PatId {
        // A pattern re-parsed from a synthesised snippet must not record the
        // snippet's own ranges: take the shifted span instead of the node.
        if self.span_offset != 0 {
            let span = Self::text_range_to_span(node.text_range());
            return self.alloc_pat(hir, span);
        }
        let name = Self::binding_name_of_pat(&hir);
        let pat_id = self.builder.alloc_pat_cst(hir, node);
        if let Some(n) = name {
            self.builder.alloc_binding(n, pat_id);
        }
        pat_id
    }

    /// Allocate a Pat::Bind from a raw token name (used when PARAM_LIST
    /// contains only tokens without wrapping pattern nodes).
    /// A binding whose source range is known.
    ///
    /// It used to be `0..0` for every typed parameter, which is a real
    /// position in the file — the first byte — so anything that navigated to
    /// a parameter's binding site landed on the top of the file, and
    /// find-all-references reported an empty range for it.
    pub(crate) fn alloc_pat_bind_at(&mut self, name: String, span: Span) -> PatId {
        let hir = Pat::Bind(name);
        self.alloc_pat(hir, span)
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

    fn shift(&self, span: Span) -> Span {
        let adj = |v: usize| -> usize {
            let shifted = v as i64 + self.span_offset;
            shifted.max(0) as usize
        };
        Span::new(adj(span.start), adj(span.end))
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
                for c in node.children() {
                    if c.kind() == SK::MAPPING_CLAUSE {
                        self.lower_cst_mapping_clause(&c);
                    } else if ast::BlockItem::can_cast(c.kind()) {
                        self.lower_cst_block_item_into(&c, &mut stmts);
                    }
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
                //
                // The base is inspected before it is lowered: `config a.b.c`
                // is a chain of field accesses, and lowering each level only
                // to discard it left two dead `Expr::Config` nodes per
                // config path in the arena — 730 of them across sail-riscv,
                // every one counted as an expression inference never typed.
                let child = node.children().next();
                let field = self.last_ident_text(node).unwrap_or_default();
                if let Some(segments) = child.as_ref().and_then(|c| self.config_path_of(c)) {
                    let mut segments = segments;
                    segments.push(field);
                    return self.alloc_expr(Expr::Config(segments), span);
                }
                let base_expr = child
                    .map(|c| self.lower_cst_expr(&c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                Expr::Field { expr: base_expr, field }
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
                // `sizeof(typ)`. The grammar reads the brackets as a type
                // now, so the argument is a *node*; the token walk below
                // sees only the keyword and the brackets it excludes and
                // came back empty, and the fallback then named the whole
                // expression — `atom(sizeof('n))` instead of `atom('n)`.
                let nexp = node
                    .children()
                    .find(|c| c.kind().is_type_node())
                    .map(|c| c.text().to_string())
                    .unwrap_or_else(|| {
                        // A recovered parse leaves the argument as bare
                        // tokens beside the keyword.
                        let text = node
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
                        if text.trim().is_empty() {
                            node.text().to_string()
                        } else {
                            text.trim().to_string()
                        }
                    });
                Expr::SizeOf { nexp: nexp.trim().to_string() }
            }
            SK::CONSTRAINT_EXPR => Expr::Constraint(node.text().to_string()),
            SK::VECTOR_UPDATE_EXPR => {
                let children: Vec<_> = node.children().collect();
                let base = children
                    .first()
                    .map(|c| self.lower_cst_expr(c))
                    .unwrap_or_else(|| self.alloc_expr(Expr::Missing, span));
                match children.len() {
                    // `[v with hi .. lo = e]`
                    4 => {
                        let hi = self.lower_cst_expr(&children[1]);
                        let lo = self.lower_cst_expr(&children[2]);
                        let value = self.lower_cst_expr(&children[3]);
                        let callee = self
                            .alloc_expr(Expr::Ident("vector_update_subrange#".to_string()), span);
                        Expr::Call { callee, args: vec![base, hi, lo, value] }
                    }
                    // `[v with i = e]`, and the multi-update spelling
                    // `[cfg with X = 0b0, W = 0b0, R = 0b0]`, which is the
                    // same thing applied repeatedly. The parser leaves the
                    // updates as a flat `idx, value, idx, value, ..` list.
                    n if n >= 3 && n % 2 == 1 => {
                        let mut acc = base;
                        for pair in children[1..].chunks_exact(2) {
                            let idx = self.lower_cst_expr(&pair[0]);
                            let value = self.lower_cst_expr(&pair[1]);
                            let callee =
                                self.alloc_expr(Expr::Ident("vector_update#".to_string()), span);
                            acc = self.alloc_expr(
                                Expr::Call { callee, args: vec![acc, idx, value] },
                                span,
                            );
                        }
                        return acc;
                    }
                    // A shape this lowering does not model. `Missing` says so;
                    // a placeholder *name* would be looked up like any other
                    // identifier and reported as unresolved.
                    _ => Expr::Missing,
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
                let mut descending =
                    Some(node.children_with_tokens().any(|el| el.kind() == SK::KW_DOWNTO));
                let mut after_in = false;
                for el in node.children_with_tokens() {
                    if el.kind() == SK::KW_IN {
                        after_in = true;
                        continue;
                    }
                    if after_in {
                        let text = el.to_string();
                        let text = text.trim();
                        if text.is_empty() {
                            continue;
                        }
                        descending = match text {
                            "inc" => Some(false),
                            "dec" => Some(true),
                            _ => None,
                        };
                        break;
                    }
                }
                Expr::Foreach { descending, pat, start, end, step, body }
            }
            SK::STRUCT_EXPR => {
                let children: Vec<_> = node.children().collect();
                let mut name = None;
                let mut fields = Vec::new();
                for child in &children {
                    // `struct S { .. }` puts the type name in a `NAME` node;
                    // the bare `Name { .. }` spelling reaches here as an
                    // `IdentExpr`.
                    if ast::IdentExpr::can_cast(child.kind())
                        || matches!(child.kind(), SK::IDENT | SK::NAME)
                    {
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
                // The span of the ascribed type, not of the whole `e : T`.
                // It used to be the latter, so anything reading it back got
                // the expression text instead of a type.
                let ty = children
                    .get(1)
                    .map(|c| c.text().to_string())
                    .unwrap_or_else(|| node.text().to_string());
                Expr::Cast { expr, ty }
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
                Expr::Var { pat, value, body }
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
                // `$[complete]` and `$[incomplete]` are the two upstream
                // acts on: `type_check.ml:2205` skips the completeness
                // check when either is present. Every other attribute is
                // unwrapped, as before, so nothing else sees a new node.
                let name = attribute_name(node);
                let keep = matches!(name.as_str(), "complete" | "incomplete");
                match node.children().next() {
                    Some(child) if !keep => return self.lower_cst_expr(&child),
                    Some(child) => {
                        let inner = self.lower_cst_expr(&child);
                        Expr::Attribute { name, expr: inner }
                    }
                    None => Expr::Attribute { name, expr: self.alloc_expr(Expr::Missing, span) },
                }
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

    /// The config path a node denotes, if it is one — `config a.b` reaches
    /// here as `Field(Field(Config([]), "a"), "b")`. Returns `None` for
    /// anything that is not rooted at a `config`.
    fn config_path_of(&self, node: &syntax::SyntaxNode) -> Option<Vec<String>> {
        use parser::SyntaxKind as SK;
        match node.kind() {
            SK::CONFIG_EXPR => {
                let segments: Vec<String> = node
                    .children_with_tokens()
                    .filter_map(|el| el.into_token())
                    .filter(|tok| tok.kind() == SK::IDENT)
                    .map(|tok| tok.text().to_string())
                    .collect();
                if segments.is_empty() {
                    Some(vec![node.text().to_string().trim().to_string()])
                } else {
                    Some(segments)
                }
            }
            SK::FIELD_ACCESS_EXPR => {
                let child = node.children().next()?;
                let mut segments = self.config_path_of(&child)?;
                segments.push(self.last_ident_text(node).unwrap_or_default());
                Some(segments)
            }
            _ => None,
        }
    }

    fn lower_cst_block_item_into(
        &mut self,
        node: &syntax::SyntaxNode,
        out: &mut Vec<crate::expr_store::hir::Statement>,
    ) {
        // An empty block item — what a trailing comma leaves behind — is not
        // a statement. Emitting a `Missing` for it put an expression in the
        // arena that nothing ever looks at; a mapping body full of
        // `A <-> 0b1,` arms accounted for a thousand of them.
        if let Some(child) = node.children().next() {
            self.lower_cst_block_expr_into(&child, out);
        }
    }

    /// Lower one statement position of a block.
    ///
    /// A `let` binding scopes over the rest of the block. The parser nests
    /// what follows inside the `LET_EXPR`, so the tail is flattened back into
    /// sibling statements rather than lowered as one `Statement::Expr`:
    /// `Expr::Let` pops its scope when it is done, which dropped `SEW`,
    /// `LMUL_pow` and `num_elem` before the rest of every vector-extension
    /// instruction body could see them.
    fn lower_cst_block_expr_into(
        &mut self,
        child: &syntax::SyntaxNode,
        out: &mut Vec<crate::expr_store::hir::Statement>,
    ) {
        let span = Self::text_range_to_span(child.text_range());
        {
            let child = child.clone();
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
                        self.lower_cst_block_expr_into(body_node, out);
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
                    // `var x = v in body` scopes the same way.
                    if let Some(body_node) = children.get(2) {
                        self.lower_cst_block_expr_into(body_node, out);
                    }
                }
                _ => {
                    let id = self.lower_cst_expr(&child);
                    out.push(crate::expr_store::hir::Statement::Expr(id));
                }
            }
        }
    }

    #[expect(dead_code)]
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
                // The annotation's span is read back out of the source, so
                // it needs the same shift as everything else this collector
                // records — a pattern re-parsed from a synthesised snippet
                // would otherwise name a slice of that snippet's offsets.
                let ty = children
                    .get(1)
                    .map(|c| c.text().to_string())
                    .unwrap_or_else(|| node.text().to_string());
                Pat::Typed { inner, ty }
            }
            SK::AS_PAT => {
                let children: Vec<_> = node.children().collect();
                let pat = children
                    .first()
                    .map(|c| self.lower_cst_pat(c))
                    .unwrap_or_else(|| self.alloc_pat(Pat::Missing, span));
                // `pat as X`. Upstream reads a *plain identifier* on the
                // right as a value binding — `P_var (pat, ATyp_id id)` is
                // rewritten to `P_as (pat, id)`, `initial_check.ml:1329` —
                // and anything else, a type variable or an application, as
                // a type pattern that binds no value.
                //
                // The grammar reads the right-hand side with
                // `parse_type_expr` (`patterns.rs:149`, matching upstream's
                // `pat1 As typ`), so the name arrives inside a `TYPE_NAMED`
                // node. `last_ident_text` looks at *direct token children*
                // only, so it answered `None` and every `as` binding was
                // lowered with an empty name: `match n { m as x => x }`
                // reported `unresolved identifier x`.
                match children.get(1) {
                    Some(rhs) if rhs.kind() == SK::TYPE_NAMED => {
                        Pat::As { pat, binding: self.first_ident_text(rhs).unwrap_or_default() }
                    }
                    Some(rhs) => Pat::AsType { pat, ty: rhs.text().to_string() },
                    // No right-hand side node — the name is a token of this
                    // one, which is how a recovered parse can leave it.
                    None => {
                        Pat::As { pat, binding: self.last_ident_text(node).unwrap_or_default() }
                    }
                }
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
                    let start = children
                        .get(1)
                        .map(|c| c.text().to_string())
                        .unwrap_or_else(|| node.text().to_string());
                    let end = children
                        .get(2)
                        .map(|c| c.text().to_string())
                        .unwrap_or_else(|| node.text().to_string());
                    Pat::RangeIndex { name, start, end }
                } else {
                    let index = children
                        .get(1)
                        .map(|c| c.text().to_string())
                        .unwrap_or_else(|| node.text().to_string());
                    Pat::Index { name, index }
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

/// Lower one `lhs [if g] <-> rhs [if g]` clause from source text.
///
/// A guarded mapping clause does not survive the parser as a `<->` infix
/// expression — `I(imm) if enabled == 0b0 <-> ..` leaves the body node
/// covering `I(imm)` alone and the rest outside it. Each piece is parsed on
/// its own, with its span shifted back onto the file, the same way a
/// function clause's guard is.
///
/// Returns the body and the three expression ids; the caller turns the two
/// sides into patterns, which needs the finished arena.
pub fn lower_mapping_clause_text(
    lhs: (&str, usize),
    rhs: (&str, usize),
    guard: Option<(&str, usize)>,
) -> Option<(Body, BodySourceMap, ExprId, ExprId, Option<ExprId>)> {
    let mut ctx = ExprCollector::new();
    let lower_piece = |ctx: &mut ExprCollector, text: &str, abs: usize| -> Option<ExprId> {
        let prefix = "function __m() = { ";
        let wrapped = format!("{prefix}{text} }}\n");
        let (root, errors) = syntax::parse_text(&wrapped);
        if !errors.is_empty() {
            return None;
        }
        let block = root.descendants().find(|n| n.kind() == parser::SyntaxKind::BLOCK_EXPR)?;
        let expr =
            block.descendants().find(|n| n != &block && syntax::ast::Expr::can_cast(n.kind()))?;
        ctx.span_offset = abs as i64 - prefix.len() as i64;
        let id = ctx.lower_cst_expr(&expr);
        ctx.span_offset = 0;
        Some(id)
    };
    let lhs_expr = lower_piece(&mut ctx, lhs.0, lhs.1)?;
    let rhs_expr = lower_piece(&mut ctx, rhs.0, rhs.1)?;
    let guard_expr = guard.and_then(|(t, o)| lower_piece(&mut ctx, t, o));
    let (store, store_source_map) = ctx.builder.finish();
    let body = Body {
        store,
        params: Box::new([]),
        body_expr: lhs_expr,
        mapping_arms: Vec::new(),
        guard: None,
    };
    Some((
        body,
        BodySourceMap { file_id: None, store: store_source_map, def_base: 0 },
        lhs_expr,
        rhs_expr,
        guard_expr,
    ))
}

/// Lower a body whose single parameter is written without parentheses.
///
/// `function clause decode 0x00 @ 0b000 @ .. = ..` leaves the parser with no
/// `PARAM_LIST` node — the pattern collapses to whichever node it started
/// with — so the clause is lowered with no parameters and nothing about the
/// pattern is checked.
///
/// The pattern is re-parsed as a match arm, which is the position the
/// grammar models these shapes in, and its spans are shifted back onto the
/// file.
pub fn lower_body_with_param_text(
    root: &syntax::SyntaxNode,
    param_text: &str,
    param_abs: usize,
) -> Option<(Body, BodySourceMap)> {
    let prefix = "function __p(x) = match x { ";
    let wrapped = format!("{prefix}{param_text} => () }}\n");
    let (proot, errors) = syntax::parse_text(&wrapped);
    if !errors.is_empty() {
        return None;
    }
    let arm_pat = proot
        .descendants()
        .find(|n| n.kind() == parser::SyntaxKind::MATCH_ARM)?
        .children()
        .find(|n| syntax::ast::Pat::can_cast(n.kind()))?;
    let mut ctx = ExprCollector::new();
    ctx.span_offset = param_abs as i64 - prefix.len() as i64;
    let pat = ctx.lower_cst_pat(&arm_pat);
    ctx.span_offset = 0;
    Some(finish_body_with_guard(ctx, vec![pat], root, None))
}

/// The bare name inside `$[name ..]`.
fn attribute_name(node: &syntax::SyntaxNode) -> String {
    use parser::SyntaxKind as SK;
    node.children_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|t| t.kind() == SK::IDENT)
        .map(|t| t.text().to_string())
        .unwrap_or_default()
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
        assert!(!body.is_empty());
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
