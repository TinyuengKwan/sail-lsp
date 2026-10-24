//! Pure helper functions shared across feature crates.
//!
//! Stage extracted from `sail_server::symbols::analysis`. None
//! of these functions take a `File` / `FileDb` — they're pure
//! transformations on text or value-types — so they can live in
//! `ide-db` as the "syntactic helpers" floor that any feature crate
//! consumes without ceremony.

// Re-export from canonical location in hir-def.
pub use hir::callable_info::{CallableSignature, Parameter};

/// Built-in Sail names → one-line Markdown documentation. Used by hover
/// and completion to surface tooltip text for things like `bits`,
/// `nat`, `register`, etc.
pub fn builtin_docs(name: &str) -> Option<&'static str> {
    match name {
        "bits" => Some("`bits('n)` is a bitvector of length `'n`. It is one of the most fundamental types in Sail."),
        "int" => Some("`int` is an arbitrary-precision integer."),
        "nat" => Some("`nat` is a non-negative arbitrary-precision integer."),
        "bool" => Some("`bool` is a boolean type with values `true` and `false`."),
        "unit" => Some("`unit` is the type with a single value `()`, similar to `void` in C or `()` in Rust."),
        "string" => Some("`string` is a sequence of characters."),
        "real" => Some("`real` is a real number type."),
        "slice" => Some("`slice(xs, start, len)` returns a window of length `len` starting at `start` from a vector or bitvector."),
        "vector_access#" => Some("Internal parser rewrite for `xs[i]`. Reads one element or bitfield field from a vector-like value."),
        "vector_subrange#" => Some("Internal parser rewrite for `xs[hi .. lo]`. Extracts a subrange from a vector or bitvector."),
        "forall" => Some("`forall` is used to introduce type variables in a declaration."),
        "scattered" => Some("`scattered` allows defining a function or union across multiple files or locations."),
        "overload" => Some("`overload` defines a common name for multiple functions or operators."),
        "register" => Some("`register` declares a piece of processor state."),
        "mapping" => Some("`mapping` defines a bidirectional translation between types (e.g., for assembly formatting)."),
        "bit" => Some("`bit` is a single binary digit (0 or 1). Equivalent to `bits(1)`."),
        "option" => Some("`option('a)` is either `Some(v)` or `None`."),
        "range" => Some("`range('lo, 'hi)` is an integer in the range `'lo` to `'hi` inclusive."),
        "atom" => Some("`atom('n)` is a singleton integer type whose value is exactly `'n`."),
        "vector" => Some("`vector('n, 'order, 'elem)` is a fixed-length vector of `'n` elements with order `'order`."),
        "list" => Some("`list('a)` is a linked list of elements of type `'a`."),
        "implicit" => Some("`implicit('n)` marks a type variable as an implicit argument inferred from context."),
        // Common operations
        "assert" => Some("`assert(condition, message)` — runtime assertion. Throws if condition is false."),
        "exit" => Some("`exit()` — terminate execution (non-local control flow)."),
        "throw" => Some("`throw(exception)` — raise an exception (non-local control flow)."),
        "unsigned" => Some("`unsigned(bv)` — interpret a bitvector as an unsigned integer."),
        "signed" => Some("`signed(bv)` — interpret a bitvector as a signed (two's complement) integer."),
        "sail_zeros" => Some("`sail_zeros('n)` — produce a zero bitvector of length `'n`."),
        "sail_ones" => Some("`sail_ones('n)` — produce an all-ones bitvector of length `'n`."),
        "sail_zero_extend" => Some("`sail_zero_extend(bv, 'n)` — zero-extend bitvector to length `'n`."),
        "sail_sign_extend" => Some("`sail_sign_extend(bv, 'n)` — sign-extend bitvector to length `'n`."),
        "replicate_bits" => Some("`replicate_bits(bv, 'n)` — repeat bitvector `'n` times."),
        "not_vec" => Some("`not_vec(bv)` — bitwise NOT of a bitvector."),
        "and_vec" => Some("`and_vec(bv1, bv2)` — bitwise AND of two bitvectors."),
        "or_vec" => Some("`or_vec(bv1, bv2)` — bitwise OR of two bitvectors."),
        "xor_vec" => Some("`xor_vec(bv1, bv2)` — bitwise XOR of two bitvectors."),
        "append" => Some("`append(bv1, bv2)` — concatenate two bitvectors."),
        "read_reg" => Some("`read_reg(reg)` — read the current value of a register."),
        "write_reg" => Some("`write_reg(reg, value)` — write a value to a register."),
        "pow2" => Some("`pow2(n)` — compute 2 raised to the power `n`."),
        "print" | "print_string" => Some("Print a string to standard output (debugging/tracing)."),
        "print_int" => Some("`print_int(prefix, n)` — print an integer with a prefix string."),
        "string_append" => Some("`string_append(s1, s2)` — concatenate two strings."),
        "None" => Some("`None` — the empty variant of `option('a)`."),
        "Some" => Some("`Some(value)` — the value-carrying variant of `option('a)`."),
        _ => None,
    }
}

/// Render a callable as an LSP completion snippet with `${1:...}`
/// placeholders for each parameter. Empty parameter lists become
/// just `name()`.
pub fn function_snippet(name: &str, params: &[Parameter]) -> String {
    if params.is_empty() {
        return format!("{name}()");
    }
    let body = params
        .iter()
        .enumerate()
        .map(|(idx, param)| {
            let placeholder = param
                .name
                .split(':')
                .next()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or("arg");
            format!("${{{}:{}}}", idx + 1, snippet_escape(placeholder))
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{name}({body})")
}

/// Strip a parameter declaration like `x : int` down to its bare name
/// (`x`). Used by inlay-hint formatting to label argument positions.
pub fn inlay_param_name(param: &str) -> &str {
    param.split(':').next().map(str::trim).filter(|name| !name.is_empty()).unwrap_or(param)
}

fn snippet_escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('$', "\\$").replace('}', "\\}")
}

/// Enrich a ParsedFile with local binding decls from CallableBodies.
/// Extracts let/var/parameter bindings from Body arenas and adds them
/// as `Scope::Local` so hover + find-references can resolve local names.
pub fn enrich_parsed_with_local_bindings(
    parsed: &mut syntax::parser_lower::ParsedFile,
    bodies: &hir::CallableBodies,
) {
    use hir::{Expr, Statement};
    use syntax::parser_lower::{Decl, DeclKind, DeclRole, Scope};

    for entry in bodies.entries() {
        let body = &entry.body;

        // Parameters — these are allocated directly into body.store.pats
        // (not through ExpressionStoreBuilder), so they may not have entries
        // in the source_map. Use a fallback span of (0,0) for missing spans.
        for &pat_id in body.params.iter() {
            collect_binding_decls_inner_with_fallback(
                body,
                pat_id,
                DeclKind::Parameter,
                parsed,
                &entry.source_map,
                Some(entry.def_span),
            );
        }

        // Walk expressions for let/var/foreach bindings
        for (_expr_id, expr) in body.iter_exprs() {
            match expr {
                Expr::Let { pat, .. } => {
                    collect_binding_decls_inner(
                        body,
                        *pat,
                        DeclKind::Let,
                        parsed,
                        &entry.source_map,
                    );
                }
                Expr::Block(stmts) => {
                    for stmt in stmts {
                        match stmt {
                            Statement::Let { pat, .. } => {
                                collect_binding_decls_inner(
                                    body,
                                    *pat,
                                    DeclKind::Let,
                                    parsed,
                                    &entry.source_map,
                                );
                            }
                            Statement::Var { pat, .. } => {
                                // Extract name from Pat::Bind
                                if let Some(hir::Pat::Bind(name)) = body.pat(*pat) {
                                    if let Some(span) = entry.source_map.pat_syntax(*pat) {
                                        parsed.decls.push(Decl {
                                            name: name.clone(),
                                            kind: DeclKind::Var,
                                            role: DeclRole::Definition,
                                            scope: Scope::Local,
                                            span,
                                            name_span: None,
                                            is_scattered: false,
                                            doc: None,
                                        });
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                // Extract name from Pat::Bind
                Expr::Foreach { pat, .. } => {
                    if let Some(hir::Pat::Bind(name)) = entry.body.pat(*pat) {
                        if let Some(span) = entry.source_map.pat_syntax(*pat) {
                            parsed.decls.push(Decl {
                                name: name.clone(),
                                kind: DeclKind::Let,
                                role: DeclRole::Definition,
                                scope: Scope::Local,
                                span,
                                name_span: None,
                                is_scattered: false,
                                doc: None,
                            });
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn collect_binding_decls_inner(
    body: &hir::Body,
    pat_id: hir::PatId,
    kind: syntax::parser_lower::DeclKind,
    parsed: &mut syntax::parser_lower::ParsedFile,
    source_map: &hir::BodySourceMap,
) {
    collect_binding_decls_inner_with_fallback(body, pat_id, kind, parsed, source_map, None);
}

fn collect_binding_decls_inner_with_fallback(
    body: &hir::Body,
    pat_id: hir::PatId,
    kind: syntax::parser_lower::DeclKind,
    parsed: &mut syntax::parser_lower::ParsedFile,
    source_map: &hir::BodySourceMap,
    fallback_span: Option<hir::Span>,
) {
    use hir::Pat;
    use syntax::parser_lower::{Decl, DeclRole, Scope};

    let Some(pat) = body.pat(pat_id) else { return };
    match pat {
        Pat::Bind(name) => {
            // Use source_map first, then fallback for params not in source_map
            let span = source_map.pat_syntax(pat_id).or(fallback_span);
            if let Some(span) = span {
                parsed.decls.push(Decl {
                    name: name.clone(),
                    kind,
                    role: DeclRole::Definition,
                    scope: Scope::Local,
                    span,
                    name_span: None,
                    is_scattered: false,
                    doc: None,
                });
            }
        }
        Pat::Tuple(items) | Pat::List(items) | Pat::Array(items) => {
            for &item in items {
                collect_binding_decls_inner_with_fallback(
                    body,
                    item,
                    kind,
                    parsed,
                    source_map,
                    fallback_span,
                );
            }
        }
        Pat::App { args, .. } => {
            for &arg in args {
                collect_binding_decls_inner_with_fallback(
                    body,
                    arg,
                    kind,
                    parsed,
                    source_map,
                    fallback_span,
                );
            }
        }
        Pat::As { pat: inner, binding } => {
            collect_binding_decls_inner_with_fallback(
                body,
                *inner,
                kind,
                parsed,
                source_map,
                fallback_span,
            );
            let span = source_map.pat_syntax(pat_id).or(fallback_span);
            if let Some(span) = span {
                parsed.decls.push(Decl {
                    name: binding.clone(),
                    kind,
                    role: DeclRole::Definition,
                    scope: Scope::Local,
                    span,
                    name_span: None,
                    is_scattered: false,
                    doc: None,
                });
            }
        }
        Pat::Typed { inner, .. } | Pat::AsType { pat: inner, .. } => {
            collect_binding_decls_inner_with_fallback(
                body,
                *inner,
                kind,
                parsed,
                source_map,
                fallback_span,
            );
        }
        Pat::Struct { fields, .. } => {
            for (_, p) in fields {
                collect_binding_decls_inner_with_fallback(
                    body,
                    *p,
                    kind,
                    parsed,
                    source_map,
                    fallback_span,
                );
            }
        }
        Pat::Infix { lhs, rhs, .. } => {
            collect_binding_decls_inner_with_fallback(
                body,
                *lhs,
                kind,
                parsed,
                source_map,
                fallback_span,
            );
            collect_binding_decls_inner_with_fallback(
                body,
                *rhs,
                kind,
                parsed,
                source_map,
                fallback_span,
            );
        }
        _ => {}
    }
}

/// The struct type a `STRUCT_EXPR` is annotated with.
///
/// From the literal's own name (`struct S { .. }`) is the caller's job;
/// this answers the *annotation* — an enclosing `let x : T = ..`, a
/// callable's return type, or an ascription `(.. : T)` that follows it.
pub fn struct_literal_type_name(lit: &syntax::SyntaxNode) -> Option<String> {
    use parser::SyntaxKind as SK;
    let parent = lit.parent()?;
    if !matches!(parent.kind(), SK::NAMED_DEF | SK::CAST_EXPR | SK::LET_EXPR | SK::CALLABLE_DEF) {
        return None;
    }
    // `let x : S = struct S { .. }` nests the annotation inside a
    // `TYPED_PAT`, so search the whole binding, not just direct children —
    // and never inside the literal.
    //
    // A parameter's own annotation is not the expected type: in
    // `function f(r : bar) = struct { .. }` with no return annotation, `bar`
    // would be read as what the literal must be.
    //
    // The *outermost* type node of each sibling, never one nested inside
    // it: `let r : Mem_read_request('n, .., RISCV_strong_access) = struct
    // { .. }` has a struct name among the type arguments, and reading the
    // innermost or the last called every field of that literal unknown.
    let candidates: Vec<syntax::SyntaxNode> = parent
        .children()
        .filter(|c| c != lit && c.kind() != SK::PARAM_LIST)
        .filter_map(|c| c.descendants().find(|d| matches!(d.kind(), SK::TYPE_NAMED | SK::TYPE_APP)))
        .collect();
    // An ascription follows what it annotates — `struct { .. } : phantom(32)`
    // — and only an ascription does. Everywhere else the annotation precedes
    // the literal, and the *last* one is the closest: a callable's return
    // type comes after everything else in its header.
    //
    // Looking after the literal in a `let` reads the *next statement's*
    // annotation, because Sail nests what follows into the binding's body:
    // `let x : S1 = struct { f1 = 0 }; var y : S2 = ..` judged the first
    // literal against `S2`.
    let after_first = parent.kind() == SK::CAST_EXPR;
    let before =
        || candidates.iter().rev().find(|c| c.text_range().end() <= lit.text_range().start());
    let after = || candidates.iter().find(|c| c.text_range().start() >= lit.text_range().end());
    let ty = if after_first { after().or_else(before) } else { before().or_else(after) }?.clone();
    // `S` is a `TYPE_NAMED` whose own token is the name; `S(1)` is a
    // `TYPE_APP` whose name is a `TYPE_NAMED` child. Searching only the
    // direct tokens found the first and not the second, so no literal
    // annotated with a parameterised struct type was ever checked.
    ty.descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|t| t.kind() == SK::IDENT)
        .map(|t| t.text().to_string())
}
