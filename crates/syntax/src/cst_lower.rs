//! CST → ParsedFile extraction.
//!
//! Walks the rowan CST (from `parsing::parse_text`) and extracts
//! the same `ParsedFile` data that `parser_lower.rs` extracts from
//! `core_ast`. This is the CST-native path .
//!
//! The old `parser_lower.rs` path remains for backwards compatibility.

use crate::ast::{self, AstNode};
use crate::syntax_node::SyntaxNode;
use parser::Span;
use parser::SyntaxKind as SK;

use crate::parser_lower::{
    CallSite, CallableHead, CallableParam, Decl, DeclKind, DeclRole, ParsedFile, Scope,
    SymbolOccurrence, SymbolOccurrenceKind, TypeAlias, TypedBinding,
};

/// Extract `ParsedFile` data from a rowan CST root.
pub fn parsed_file_from_cst(root: &SyntaxNode, text: &str) -> ParsedFile {
    let mut parsed = ParsedFile::default();

    for child in root.children() {
        lower_definition(&child, &mut parsed, text);
    }

    // Dedup symbol_occurrences: if two occurrences share the same span,
    // keep the one with more info (scope/role set). This handles the case
    // where lower_callable_def records a TopLevel occurrence for the name
    // AND collect_symbols_in_children also records an unscoped one.
    parsed.symbol_occurrences.sort_by_key(|o| (o.span.start, o.span.end));
    parsed.symbol_occurrences.dedup_by(|b, a| {
        if a.span == b.span && a.name == b.name && a.kind == b.kind {
            // Keep the one with more info (scope set, role set)
            if b.scope.is_some() && a.scope.is_none() {
                *a = b.clone();
            }
            true
        } else {
            false
        }
    });

    parsed
}

fn lower_definition(node: &SyntaxNode, parsed: &mut ParsedFile, text: &str) {
    let kind = node.kind();
    let span = node_span(node);

    match kind {
        SK::CALLABLE_DEF => lower_callable_def(node, parsed, text),
        SK::CALLABLE_SPEC => lower_callable_spec(node, parsed, text),
        SK::TYPE_ALIAS_DEF => lower_type_alias(node, parsed),
        SK::NAMED_DEF => lower_named_def(node, parsed, text),
        SK::SCATTERED_DEF => {
            if let Some(name) = first_ident(node) {
                push_decl(parsed, DeclKind::Function, DeclRole::Declaration, &name, span, true);
                // E4: Push SymbolOccurrence for scattered head so rename finds it.
                if let Some(ns) = first_ident_span(node) {
                    parsed.symbol_occurrences.push(SymbolOccurrence {
                        name: name.clone(),
                        kind: SymbolOccurrenceKind::Value,
                        span: ns,
                        scope: Some(Scope::TopLevel),
                        role: Some(DeclRole::Declaration),
                        target_span: Some(ns),
                    });
                }
            }
        }
        SK::SCATTERED_CLAUSE_DEF => {
            if let Some(name) = first_ident(node) {
                push_decl(parsed, DeclKind::Function, DeclRole::Definition, &name, span, true);
                // E4: Also push SymbolOccurrence so rename finds scattered clauses.
                // Without this, rename_edits misses clause names in other files.
                if let Some(ns) = first_ident_span(node) {
                    parsed.symbol_occurrences.push(SymbolOccurrence {
                        name: name.clone(),
                        kind: SymbolOccurrenceKind::Value,
                        span: ns,
                        scope: Some(Scope::TopLevel),
                        role: Some(DeclRole::Definition),
                        target_span: Some(ns),
                    });
                }
            }
        }
        SK::END_DEF => {
            // Push SymbolOccurrence for `end foo` markers
            // so rename_edits finds and renames them along with the
            // scattered head and clauses. Without this, `end foo` is
            // missed during rename of scattered function `foo`.
            if let Some(name) = first_ident(node) {
                if let Some(ns) = first_ident_span(node) {
                    parsed.symbol_occurrences.push(SymbolOccurrence {
                        name: name.clone(),
                        kind: SymbolOccurrenceKind::Value,
                        span: ns,
                        scope: Some(Scope::TopLevel),
                        role: None, // reference to the scattered head, not a definition
                        target_span: None,
                    });
                }
            }
        }
        _ => {}
    }
}

fn lower_callable_def(node: &SyntaxNode, parsed: &mut ParsedFile, _text: &str) {
    let span = node_span(node);
    let name = match first_ident(node) {
        Some(n) => n,
        None => return,
    };

    // Determine kind from first keyword
    let first_kw = first_keyword(node);
    let decl_kind = match first_kw.as_deref() {
        Some("mapping") => DeclKind::Mapping,
        _ => DeclKind::Function,
    };

    push_decl(parsed, decl_kind, DeclRole::Definition, &name, span, false);

    // Record function name as top-level symbol occurrence for resolve_symbol_at.
    if let Some(ns) = first_ident_span(node) {
        parsed.symbol_occurrences.push(SymbolOccurrence {
            name: name.clone(),
            kind: SymbolOccurrenceKind::Value,
            span: ns,
            scope: Some(Scope::TopLevel),
            role: Some(DeclRole::Definition),
            target_span: Some(ns),
        });
    }

    // Extract callable head
    let name_span = first_ident_span(node).unwrap_or(span);
    let params = extract_params(node);

    // Record typed bindings for function parameters with type annotations.
    // `f(x : bits(32), y : int)` → typed_bindings[x] = "bits(32)", [y] = "int"
    for param in &params {
        if let (Some(ref pname), Some(ty_span)) = (&param.name, param.ty_span) {
            parsed.typed_bindings.push(TypedBinding {
                name: pname.clone(),
                name_span: param.name_span.unwrap_or(param.span),
                ty_span,
                scope: Scope::Local,
            });
        }
    }

    let return_type_span = find_return_type_span(node);
    parsed.callable_heads.push(CallableHead {
        name: name.clone(),
        kind: decl_kind,
        name_span,
        label_span: span,
        params,
        return_type_span,
    });

    // Walk expression children for symbol occurrences
    collect_symbols_in_children(node, &Some(name), parsed);
}

fn lower_callable_spec(node: &SyntaxNode, parsed: &mut ParsedFile, _text: &str) {
    let span = node_span(node);
    let name = match first_ident(node) {
        Some(n) => n,
        None => return,
    };

    let first_kw = first_keyword(node);
    let decl_kind = match first_kw.as_deref() {
        Some("mapping") => DeclKind::Mapping,
        _ => DeclKind::Value,
    };

    push_decl(parsed, decl_kind, DeclRole::Declaration, &name, span, false);

    let name_span = first_ident_span(node).unwrap_or(span);
    // Record val name as top-level symbol occurrence for resolve_symbol_at.
    parsed.symbol_occurrences.push(SymbolOccurrence {
        name: name.clone(),
        kind: SymbolOccurrenceKind::Value,
        span: name_span,
        scope: Some(Scope::TopLevel),
        role: Some(DeclRole::Declaration),
        target_span: Some(name_span),
    });

    let return_type_span = find_return_type_span(node);
    // For val specs, also extract params from the type signature
    let params = extract_spec_params(node);
    parsed.callable_heads.push(CallableHead {
        name: name.clone(),
        kind: decl_kind,
        name_span,
        label_span: span,
        params,
        return_type_span,
    });

    // Walk type signature for symbol occurrences (type vars like 'n).
    // All tokens in a definition are indexed for find-references.
    collect_symbols_in_children(node, &None, parsed);
}

fn lower_type_alias(node: &SyntaxNode, parsed: &mut ParsedFile) {
    let span = node_span(node);
    let Some(name) = first_ident(node) else {
        return;
    };
    push_decl(parsed, DeclKind::Type, DeclRole::Definition, &name, span, false);

    // Extract the type alias target: `type child = parent`
    // The target type is the first TYPE_* child node after the EQ token.
    let mut found_eq = false;
    for el in node.children_with_tokens() {
        if let Some(tok) = el.as_token() {
            if tok.kind() == SK::EQ {
                found_eq = true;
            }
        }
        if found_eq {
            if let Some(n) = el.as_node() {
                if n.kind().is_type_node() {
                    // Extract the target type name from the node text
                    let sup = first_ident_in(n)
                        .unwrap_or_else(|| n.text().to_string().trim().to_string());
                    if !sup.is_empty() {
                        parsed.type_aliases.push(TypeAlias { sub: name.clone(), sup, span });
                    }
                    break;
                }
            }
        }
    }
}

fn first_ident_in(node: &SyntaxNode) -> Option<String> {
    node.children_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|t| t.kind() == SK::IDENT)
        .map(|t| t.text().to_string())
}

fn lower_named_def(node: &SyntaxNode, parsed: &mut ParsedFile, _text: &str) {
    let span = node_span(node);
    // For let/var, the binding name might be `_` (UNDERSCORE token, not IDENT).
    // `_` is a valid wildcard pattern in let bindings.
    let name = first_ident_or_underscore(node);
    let Some(name) = name else { return };

    let first_kw = first_keyword(node);
    let decl_kind = match first_kw.as_deref() {
        Some("struct") => DeclKind::Struct,
        Some("enum") => DeclKind::Enum,
        Some("union") => DeclKind::Union,
        Some("bitfield") => DeclKind::Bitfield,
        Some("newtype") => DeclKind::Newtype,
        Some("register") => DeclKind::Register,
        Some("let") => DeclKind::Let,
        Some("var") => DeclKind::Var,
        Some("overload") => DeclKind::Overload,
        _ => DeclKind::Value,
    };

    // For let/var, record name_span for type inference PatId matching.
    let ns = first_ident_span(node);
    if let Some(ns) = ns {
        push_decl_with_name_span(parsed, decl_kind, DeclRole::Definition, &name, span, ns, false);
    } else {
        push_decl(parsed, decl_kind, DeclRole::Definition, &name, span, false);
    }

    // For enums, extract enum members
    if decl_kind == DeclKind::Enum {
        extract_enum_members(node, parsed);
    }
    // For unions, extract constructor names (IDENT tokens after `=`)
    if decl_kind == DeclKind::Union {
        extract_union_constructors(node, parsed);
    }

    // For let/var with type annotation, extract typed binding.
    // `let x : child = y` → typed_bindings[x] = "child"
    if matches!(decl_kind, DeclKind::Let | DeclKind::Var) {
        let name_span = first_ident_span(node).unwrap_or(span);
        // Find TYPE_* child after COLON
        let mut found_colon = false;
        for el in node.children_with_tokens() {
            if let Some(tok) = el.as_token() {
                if tok.kind() == SK::COLON {
                    found_colon = true;
                }
                if tok.kind() == SK::EQ {
                    break;
                }
            }
            if found_colon {
                if let Some(n) = el.as_node() {
                    if n.kind().is_type_node() {
                        parsed.typed_bindings.push(TypedBinding {
                            name: name.clone(),
                            name_span,
                            ty_span: node_span(n),
                            scope: Scope::TopLevel,
                        });
                        break;
                    }
                }
            }
        }
    }

    // Walk expression children for symbol occurrences + call sites.
    if matches!(decl_kind, DeclKind::Let | DeclKind::Var | DeclKind::Value) {
        collect_symbols_in_children(node, &None, parsed);
    }
}

/// J3-4: Extract enum members with doc comments.
///
/// Upstream Sail (commit f7a67174) allows `///` doc comments on
/// enum members.
fn extract_enum_members(node: &SyntaxNode, parsed: &mut ParsedFile) {
    // Enum members are IDENT tokens after `=`.
    // Two forms:
    //   1. `enum foo = { A, B, C }` (braces + commas)
    //   2. `enum foo = A | B | C`   (pipe-separated)
    // Both: collect IDENT tokens after the `=` token, skipping the enum name.
    let mut after_eq = false;
    let enum_name = first_ident(node);
    // J3-4: Accumulate doc comment lines preceding each member.
    let mut pending_doc: Option<String> = None;
    for tok_or_node in node.descendants_with_tokens() {
        if let Some(tok) = tok_or_node.as_token() {
            if tok.kind() == SK::EQ {
                after_eq = true;
                continue;
            }
            // Collect /// doc comments
            if after_eq && tok.kind() == SK::DOC_COMMENT {
                let text = tok.text().trim_start_matches('/').trim();
                match &mut pending_doc {
                    Some(doc) => {
                        doc.push('\n');
                        doc.push_str(text);
                    }
                    None => {
                        pending_doc = Some(text.to_string());
                    }
                }
                continue;
            }
            // Reset doc accumulation on non-trivia, non-doc tokens
            // (commas, pipes, braces clear pending docs unless followed by IDENT)
            if after_eq && tok.kind() == SK::IDENT {
                let name = tok.text().to_string();
                // Skip the enum name itself
                if Some(&name) == enum_name.as_ref() {
                    pending_doc = None;
                    continue;
                }
                let span = token_span(tok);
                let doc = pending_doc.take();
                parsed.decls.push(Decl {
                    name: name.clone(),
                    kind: DeclKind::EnumMember,
                    role: DeclRole::Definition,
                    scope: Scope::TopLevel,
                    span,
                    name_span: None,
                    is_scattered: false,
                    doc,
                });
                parsed.symbol_occurrences.push(SymbolOccurrence {
                    name,
                    kind: SymbolOccurrenceKind::Value,
                    span,
                    scope: Some(Scope::TopLevel),
                    role: Some(DeclRole::Definition),
                    target_span: None,
                });
            } else if after_eq && !tok.kind().is_trivia() && tok.kind() != SK::DOC_COMMENT {
                // Non-trivia, non-doc, non-IDENT: clear pending doc
                pending_doc = None;
            }
        }
    }
}

/// Extract union constructor (variant) names from a NAMED_DEF with `union` keyword.
/// Union bodies have the form `{ Ctor1 : Type1, Ctor2 : Type2 }`.
fn extract_union_constructors(node: &SyntaxNode, parsed: &mut ParsedFile) {
    let mut in_braces = false;
    let mut expect_name = true; // alternate: name, then skip until comma
    for tok_or_node in node.descendants_with_tokens() {
        if let Some(tok) = tok_or_node.as_token() {
            match tok.kind() {
                SK::L_CURLY => {
                    in_braces = true;
                    expect_name = true;
                }
                SK::R_CURLY => in_braces = false,
                SK::COMMA if in_braces => expect_name = true,
                SK::IDENT if in_braces && expect_name => {
                    parsed.union_constructor_names.push(tok.text().to_string());
                    expect_name = false;
                }
                _ => {}
            }
        }
    }
}

/// Scope-tracking expression walker. Collects symbol occurrences with
/// scope/role info, call sites, and typed bindings.
struct ExprWalker<'a> {
    parsed: &'a mut ParsedFile,
    caller: Option<String>,
    /// Stack of local binding scopes. Each entry maps binding name → span.
    local_scopes: Vec<std::collections::HashMap<String, Span>>,
}

impl<'a> ExprWalker<'a> {
    fn new(parsed: &'a mut ParsedFile, caller: Option<String>) -> Self {
        Self { parsed, caller, local_scopes: Vec::new() }
    }

    fn push_scope(&mut self) {
        self.local_scopes.push(std::collections::HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.local_scopes.pop();
    }

    fn add_binding(&mut self, name: &str, span: Span) {
        if let Some(scope) = self.local_scopes.last_mut() {
            scope.insert(name.to_string(), span);
        }
    }

    fn resolve_local(&self, name: &str) -> Option<Span> {
        for scope in self.local_scopes.iter().rev() {
            if let Some(&span) = scope.get(name) {
                return Some(span);
            }
        }
        None
    }

    fn walk_node(&mut self, node: &SyntaxNode) {
        match node.kind() {
            SK::CALL_EXPR => self.walk_call(node),
            SK::LET_EXPR => self.walk_let(node),
            SK::VAR_EXPR => self.walk_var(node),
            SK::MATCH_EXPR | SK::TRY_EXPR => self.walk_match(node),
            SK::BLOCK_EXPR => self.walk_block(node),
            SK::FOREACH_EXPR => self.walk_foreach(node),
            _ => {
                // Collect ident/tyvar tokens from this node
                self.collect_tokens(node);
                // Recurse into child nodes
                for child in node.children() {
                    self.walk_node(&child);
                }
            }
        }
    }

    fn collect_tokens(&mut self, node: &SyntaxNode) {
        for tok_or_node in node.children_with_tokens() {
            if let Some(tok) = tok_or_node.as_token() {
                let tk = tok.kind();
                if tk == SK::IDENT {
                    let name = tok.text().to_string();
                    let span = token_span(tok);
                    let target = self.resolve_local(&name);
                    let scope = if target.is_some() { Some(Scope::Local) } else { None };
                    self.parsed.symbol_occurrences.push(SymbolOccurrence {
                        name,
                        kind: SymbolOccurrenceKind::Value,
                        span,
                        scope,
                        role: None,
                        target_span: target,
                    });
                } else if tk == SK::TY_VAR {
                    let name = tok.text().to_string();
                    let span = token_span(tok);
                    self.parsed.symbol_occurrences.push(SymbolOccurrence {
                        name,
                        kind: SymbolOccurrenceKind::TypeVar,
                        span,
                        scope: None,
                        role: None,
                        target_span: None,
                    });
                }
            }
        }
    }

    fn walk_call(&mut self, node: &SyntaxNode) {
        // Extract call site
        let children: Vec<_> = node.children().collect();
        if let Some(callee_node) = children.first() {
            let callee_name = first_ident(callee_node);
            if let Some(callee) = callee_name {
                let callee_span = node_span(callee_node);
                // Find parens + comma spans. Arguments may be wrapped in
                // an ARG_LIST node, so search descendants (not just direct children).
                let mut open_span = callee_span;
                let mut close_span = None;
                let mut arg_sep_spans = Vec::new();
                for tok_or_node in node.descendants_with_tokens() {
                    if let Some(tok) = tok_or_node.as_token() {
                        match tok.kind() {
                            SK::L_PAREN => open_span = token_span(tok),
                            SK::R_PAREN => close_span = Some(token_span(tok)),
                            SK::COMMA => arg_sep_spans.push(token_span(tok)),
                            _ => {}
                        }
                    }
                }
                self.parsed.call_sites.push(CallSite {
                    caller: self.caller.clone(),
                    callee,
                    callee_span,
                    open_span,
                    close_span,
                    arg_separator_spans: arg_sep_spans,
                });
            }
        }
        // Walk children normally
        self.collect_tokens(node);
        for child in node.children() {
            self.walk_node(&child);
        }
    }

    fn walk_let(&mut self, node: &SyntaxNode) {
        let children: Vec<_> = node.children().collect();
        self.push_scope();
        // First child is pattern — extract bindings
        if let Some(pat_node) = children.first() {
            self.extract_pattern_bindings(pat_node);
        }
        // Walk value and body
        for child in children.iter().skip(1) {
            self.walk_node(child);
        }
        self.pop_scope();
    }

    fn walk_var(&mut self, node: &SyntaxNode) {
        let children: Vec<_> = node.children().collect();
        self.push_scope();
        if let Some(pat_node) = children.first() {
            self.extract_pattern_bindings(pat_node);
        }
        for child in children.iter().skip(1) {
            self.walk_node(child);
        }
        self.pop_scope();
    }

    fn walk_match(&mut self, node: &SyntaxNode) {
        // Walk scrutinee
        let children: Vec<_> = node.children().collect();
        if let Some(scrutinee) = children.first() {
            self.walk_node(scrutinee);
        }
        // Walk each arm with its own scope
        for child in node.children() {
            if ast::MatchArm::can_cast(child.kind()) {
                self.push_scope();
                let arm_children: Vec<_> = child.children().collect();
                // First child is pattern
                if let Some(pat_node) = arm_children.first() {
                    self.extract_pattern_bindings(pat_node);
                }
                // Rest are guard + body
                for arm_child in arm_children.iter().skip(1) {
                    self.walk_node(arm_child);
                }
                self.pop_scope();
            }
        }
    }

    fn walk_block(&mut self, node: &SyntaxNode) {
        self.push_scope();
        for child in node.children() {
            if ast::BlockItem::can_cast(child.kind()) {
                // Check if block item contains a let
                for inner in child.children() {
                    if ast::LetExpr::can_cast(inner.kind()) {
                        let let_children: Vec<_> = inner.children().collect();
                        if let Some(pat_node) = let_children.first() {
                            self.extract_pattern_bindings(pat_node);
                        }
                        for c in let_children.iter().skip(1) {
                            self.walk_node(c);
                        }
                    } else {
                        self.walk_node(&inner);
                    }
                }
            } else {
                self.walk_node(&child);
            }
        }
        self.pop_scope();
    }

    fn walk_foreach(&mut self, node: &SyntaxNode) {
        self.push_scope();
        // First ident in foreach is the iterator variable
        if let Some(iter_name) = first_ident(node) {
            if let Some(iter_span) = first_ident_span(node) {
                self.add_binding(&iter_name, iter_span);
                self.parsed.symbol_occurrences.push(SymbolOccurrence {
                    name: iter_name,
                    kind: SymbolOccurrenceKind::Value,
                    span: iter_span,
                    scope: Some(Scope::Local),
                    role: Some(DeclRole::Definition),
                    target_span: Some(iter_span),
                });
            }
        }
        for child in node.children() {
            self.walk_node(&child);
        }
        self.pop_scope();
    }

    /// Extract binding names from a pattern node and add them to the
    /// current scope. Also records typed bindings when `: type` is present.
    fn extract_pattern_bindings(&mut self, pat_node: &SyntaxNode) {
        match pat_node.kind() {
            SK::IDENT_PAT => {
                if let Some(name) = first_ident(pat_node) {
                    // Use the IDENT token span (not the IDENT_PAT node span)
                    // so it matches token_at() for resolve_symbol_at.
                    let span = first_ident_span(pat_node).unwrap_or_else(|| node_span(pat_node));
                    // Check if this identifier is a known top-level enum member.
                    // If so, it's a pattern match on the enum variant, not a
                    // new local binding. RA handles this via name resolution
                    // in the Resolver; we approximate by checking parsed.decls.
                    let is_enum_member = self
                        .parsed
                        .decls
                        .iter()
                        .any(|d| d.name == name && d.kind == DeclKind::EnumMember);
                    if is_enum_member {
                        self.parsed.symbol_occurrences.push(SymbolOccurrence {
                            name,
                            kind: SymbolOccurrenceKind::Value,
                            span,
                            scope: Some(Scope::TopLevel),
                            role: None,
                            target_span: None,
                        });
                    } else {
                        self.add_binding(&name, span);
                        self.parsed.symbol_occurrences.push(SymbolOccurrence {
                            name,
                            kind: SymbolOccurrenceKind::Value,
                            span,
                            scope: Some(Scope::Local),
                            role: Some(DeclRole::Definition),
                            target_span: Some(span),
                        });
                    }
                }
            }
            SK::TYPED_PAT => {
                // Inner pattern + type annotation
                let children: Vec<_> = pat_node.children().collect();
                if let Some(inner) = children.first() {
                    self.extract_pattern_bindings(inner);
                    // Record typed binding
                    if let Some(name) = first_ident(inner) {
                        let name_span = node_span(inner);
                        let ty_span = if children.len() > 1 {
                            node_span(&children[1])
                        } else {
                            node_span(pat_node)
                        };
                        self.parsed.typed_bindings.push(TypedBinding {
                            name,
                            name_span,
                            ty_span,
                            scope: Scope::Local,
                        });
                    }
                }
            }
            SK::TUPLE_PAT | SK::LIST_PAT | SK::VECTOR_PAT => {
                for child in pat_node.children() {
                    self.extract_pattern_bindings(&child);
                }
            }
            SK::APP_PAT => {
                // Constructor pattern: first child is ctor name, rest are args
                let children: Vec<_> = pat_node.children().collect();
                if let Some(ctor_node) = children.first() {
                    if let Some(name) = first_ident(ctor_node) {
                        let span =
                            first_ident_span(ctor_node).unwrap_or_else(|| node_span(ctor_node));
                        self.parsed.symbol_occurrences.push(SymbolOccurrence {
                            name,
                            kind: SymbolOccurrenceKind::Value,
                            span,
                            scope: Some(Scope::TopLevel),
                            role: None,
                            target_span: None,
                        });
                    }
                }
                for child in children.iter().skip(1) {
                    self.extract_pattern_bindings(child);
                }
            }
            SK::AS_PAT => {
                // pat as binding — extract from inner pat and binding name
                let children: Vec<_> = pat_node.children().collect();
                if let Some(inner) = children.first() {
                    self.extract_pattern_bindings(inner);
                }
                // The `as` binding name is the last IDENT token
                if let Some(name) = last_ident(pat_node) {
                    let span = node_span(pat_node);
                    self.add_binding(&name, span);
                }
            }
            SK::STRUCT_PAT => {
                for child in pat_node.children() {
                    if ast::FieldInit::can_cast(child.kind()) {
                        for inner in child.children() {
                            self.extract_pattern_bindings(&inner);
                        }
                    }
                }
            }
            SK::BIN_PAT => {
                for child in pat_node.children() {
                    self.extract_pattern_bindings(&child);
                }
            }
            _ => {}
        }
    }
}

fn collect_symbols_in_children(
    node: &SyntaxNode,
    caller: &Option<String>,
    parsed: &mut ParsedFile,
) {
    let mut walker = ExprWalker::new(parsed, caller.clone());
    for child in node.children() {
        walker.walk_node(&child);
    }
}

fn last_ident(node: &SyntaxNode) -> Option<String> {
    node.descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .filter(|t| t.kind() == SK::IDENT)
        .last()
        .map(|t| t.text().to_string())
}

/// Like first_ident but also accepts UNDERSCORE (`_`) for wildcard let/var.
/// Only searches the NAME child node (not the entire subtree) to avoid
/// picking up identifiers from expression bodies.
fn first_ident_or_underscore(node: &SyntaxNode) -> Option<String> {
    // Try NAME child first (structured name node)
    for child in node.children() {
        if ast::Name::can_cast(child.kind()) {
            for tok in child.children_with_tokens().filter_map(|el| el.into_token()) {
                if tok.kind() == SK::IDENT {
                    return Some(tok.text().to_string());
                }
                if tok.kind() == SK::UNDERSCORE {
                    return Some("_".to_string());
                }
            }
        }
    }
    // Fallback: first IDENT or UNDERSCORE among direct token children
    for tok in node.children_with_tokens().filter_map(|el| el.into_token()) {
        if tok.kind() == SK::IDENT {
            return Some(tok.text().to_string());
        }
        if tok.kind() == SK::UNDERSCORE {
            return Some("_".to_string());
        }
    }
    None
}

fn push_decl(
    parsed: &mut ParsedFile,
    kind: DeclKind,
    role: DeclRole,
    name: &str,
    span: Span,
    is_scattered: bool,
) {
    parsed.decls.push(Decl {
        name: name.to_string(),
        kind,
        role,
        scope: Scope::TopLevel,
        span,
        name_span: None,
        is_scattered,
        doc: None,
    });
}

fn push_decl_with_name_span(
    parsed: &mut ParsedFile,
    kind: DeclKind,
    role: DeclRole,
    name: &str,
    span: Span,
    name_span: Span,
    is_scattered: bool,
) {
    parsed.decls.push(Decl {
        name: name.to_string(),
        kind,
        role,
        scope: Scope::TopLevel,
        span,
        name_span: Some(name_span),
        is_scattered,
        doc: None,
    });
}

fn node_span(node: &SyntaxNode) -> Span {
    let range = node.text_range();
    Span::new(u32::from(range.start()) as usize, u32::from(range.end()) as usize)
}

fn token_span(tok: &crate::syntax_node::SyntaxToken) -> Span {
    let range = tok.text_range();
    Span::new(u32::from(range.start()) as usize, u32::from(range.end()) as usize)
}

fn first_ident(node: &SyntaxNode) -> Option<String> {
    node.descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|t| t.kind() == SK::IDENT)
        .map(|t| t.text().to_string())
}

fn first_ident_span(node: &SyntaxNode) -> Option<Span> {
    node.descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|t| t.kind() == SK::IDENT)
        .map(|t| token_span(&t))
}

fn first_keyword(node: &SyntaxNode) -> Option<String> {
    node.children_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|t| {
            let k = t.kind();
            !k.is_trivia() && k != SK::IDENT && k != SK::TY_VAR
        })
        .map(|t| t.text().to_string())
}

fn extract_params(node: &SyntaxNode) -> Vec<CallableParam> {
    // Look for IDENT tokens + type annotations inside the first PARAM_LIST child.
    // PARAM_LIST contains: IDENT COLON TYPE_* COMMA IDENT COLON TYPE_* ...
    // We track state to pair each IDENT with its following type annotation.
    let mut params = Vec::new();
    for child in node.children() {
        if ast::ParamList::can_cast(child.kind()) {
            let mut current_name: Option<(String, Span)> = None;
            let mut after_colon = false;

            for el in child.children_with_tokens() {
                match el {
                    rowan::NodeOrToken::Token(tok) => {
                        let kind = tok.kind();
                        if kind == SK::IDENT && !after_colon {
                            // Finalize previous param if any
                            if let Some((name, name_span)) = current_name.take() {
                                params.push(CallableParam {
                                    span: name_span,
                                    name: Some(name),
                                    name_span: Some(name_span),
                                    ty_span: None,
                                });
                            }
                            current_name = Some((tok.text().to_string(), token_span(&tok)));
                            after_colon = false;
                        } else if kind == SK::COLON {
                            after_colon = true;
                        } else if kind == SK::COMMA || kind == SK::R_PAREN {
                            // Finalize current param
                            if let Some((name, name_span)) = current_name.take() {
                                params.push(CallableParam {
                                    span: name_span,
                                    name: Some(name),
                                    name_span: Some(name_span),
                                    ty_span: None,
                                });
                            }
                            after_colon = false;
                        }
                    }
                    rowan::NodeOrToken::Node(n) => {
                        if after_colon && n.kind().is_type_node() {
                            // This type node is the annotation for current_name
                            if let Some((name, name_span)) = current_name.take() {
                                params.push(CallableParam {
                                    span: Span::new(name_span.start, node_span(&n).end),
                                    name: Some(name),
                                    name_span: Some(name_span),
                                    ty_span: Some(node_span(&n)),
                                });
                            }
                            after_colon = false;
                        }
                    }
                }
            }
            // Finalize last param if it wasn't terminated by comma/rparen
            if let Some((name, name_span)) = current_name.take() {
                params.push(CallableParam {
                    span: name_span,
                    name: Some(name),
                    name_span: Some(name_span),
                    ty_span: None,
                });
            }
            break; // first PARAM_LIST only
        }
    }
    params
}

fn find_return_type_span(node: &SyntaxNode) -> Option<Span> {
    // For val specs: the return type is the rightmost leaf of the arrow chain.
    // `int -> bool -> string` → return type = "string" (last non-arrow child)
    // `(int, bool) -> string` → return type = "string"
    //
    // For function defs: `function f(x) -> int = ...` → return type = "int"
    // (arrow is direct child, not inside TYPE_ARROW)

    // Strategy: find the outermost TYPE_ARROW (may be inside TYPE_FORALL), walk to its rightmost leaf.
    let arrow = node.children().find(|c| ast::TypeArrow::can_cast(c.kind())).or_else(|| {
        node.children()
            .filter(|c| {
                ast::TypeForall::can_cast(c.kind()) || ast::TypeExistential::can_cast(c.kind())
            })
            .flat_map(|c| c.children())
            .find(|c| ast::TypeArrow::can_cast(c.kind()))
    });
    if let Some(arrow) = arrow {
        let ret = find_arrow_return_type(&arrow);
        if ret.is_some() {
            return ret;
        }
    }

    // Fallback for function defs: `function f(x) -> int = body`
    // Here `->` is a direct child token, not inside TYPE_ARROW.
    let mut found_arrow = false;
    let mut ret_start = None;
    let mut ret_end = None;
    for el in node.children_with_tokens() {
        if let Some(tok) = el.as_token() {
            if tok.kind() == SK::R_ARROW {
                found_arrow = true;
                continue;
            }
            if found_arrow {
                if tok.kind() == SK::EQ {
                    break;
                }
                if tok.kind().is_trivia() {
                    continue;
                }
                let ts = token_span(tok);
                if ret_start.is_none() {
                    ret_start = Some(ts.start);
                }
                ret_end = Some(ts.end);
            }
        }
        if let Some(n) = el.as_node() {
            if found_arrow && n.kind().is_type_node() {
                return Some(node_span(n));
            }
        }
    }
    match (ret_start, ret_end) {
        (Some(s), Some(e)) => Some(Span::new(s, e)),
        _ => None,
    }
}

/// Find the return type of a TYPE_ARROW chain — the rightmost leaf type.
fn find_arrow_return_type(arrow: &SyntaxNode) -> Option<Span> {
    // TYPE_ARROW children: [lhs, R_ARROW, rhs]
    // If rhs is TYPE_ARROW → recurse (right-associative chain)
    // If rhs is any other type → that's the return type
    let children: Vec<_> = arrow.children().collect();
    // Last child is the RHS
    if let Some(last) = children.last() {
        if ast::TypeArrow::can_cast(last.kind()) {
            return find_arrow_return_type(last);
        }
        if last.kind().is_type_node() {
            return Some(node_span(last));
        }
    }
    None
}

/// Extract parameter types from a val spec type signature.
///
/// For `val f : int -> bool -> string`, extracts [int, bool] as params
/// (the last type in the arrow chain is the return type).
///
/// For `val add : (int, bool) -> string`, extracts [int, bool] from the tuple.
///
/// Val specs don't have named parameters — names come from function defs.
/// This.
/// (`hir-def/body/lower.rs`).
fn extract_spec_params(node: &SyntaxNode) -> Vec<CallableParam> {
    // Find the TYPE_ARROW inside the CALLABLE_SPEC. It may be a direct
    // child or wrapped in TYPE_FORALL (e.g., `forall 'n. unit -> bits('n)`).
    let arrow = node.children().find(|c| ast::TypeArrow::can_cast(c.kind())).or_else(|| {
        // Look inside TYPE_FORALL / TYPE_EXISTENTIAL wrappers
        node.children()
            .filter(|c| {
                ast::TypeForall::can_cast(c.kind()) || ast::TypeExistential::can_cast(c.kind())
            })
            .flat_map(|c| c.children())
            .find(|c| ast::TypeArrow::can_cast(c.kind()))
    });
    let Some(arrow) = arrow else {
        return Vec::new();
    };

    // Collect all param types from the arrow chain.
    // `int -> bool -> string` → params = [int, bool], return = string
    // `(int, bool) -> string` → params = [int, bool], return = string
    let mut param_types = Vec::new();
    collect_arrow_param_types(&arrow, &mut param_types);
    param_types
}

/// Walk a TYPE_ARROW chain and collect all parameter types (everything
/// except the final return type).
///
/// TYPE_ARROW structure (right-associative):
///   TYPE_ARROW { lhs, R_ARROW, rhs }
/// where rhs may itself be TYPE_ARROW for multi-param signatures.
fn collect_arrow_param_types(arrow: &SyntaxNode, out: &mut Vec<CallableParam>) {
    assert_eq!(arrow.kind(), SK::TYPE_ARROW);

    // Children of TYPE_ARROW: [lhs_type, R_ARROW, rhs_type]
    // lhs_type is the parameter(s), rhs_type is the rest of the chain.
    let mut children = arrow.children().peekable();

    // First child = LHS (param type or tuple of param types)
    let Some(lhs) = children.next() else { return };

    if ast::TypeTuple::can_cast(lhs.kind()) {
        // (int, bool) → expand each element as a separate param
        for child in lhs.children() {
            if child.kind().is_type_node() {
                out.push(type_node_to_param(&child));
            }
        }
    } else if lhs.kind().is_type_node() {
        // Single param type: int -> ...
        out.push(type_node_to_param(&lhs));
    }

    // Find the RHS. Skip until we find the next type node after R_ARROW.
    for child in children {
        if ast::TypeArrow::can_cast(child.kind()) {
            // Recurse into nested arrow: bool -> string
            collect_arrow_param_types(&child, out);
            return;
        }
        // If RHS is a non-arrow type, it's the return type — stop.
    }
}

fn type_node_to_param(node: &SyntaxNode) -> CallableParam {
    let span = node_span(node);
    CallableParam {
        span,
        name: None, // val specs don't have named params
        name_span: None,
        ty_span: Some(span),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::parse_text;

    #[test]
    fn val_spec_extracts_arrow_params() {
        // int -> bool -> string  → params = [int, bool], return = string
        let (root, _) = parse_text("val f : int -> bool -> string\n");
        let parsed = parsed_file_from_cst(&root, "val f : int -> bool -> string\n");
        assert_eq!(parsed.callable_heads.len(), 1);
        let head = &parsed.callable_heads[0];
        assert_eq!(head.params.len(), 2, "expected 2 params, got {:?}", head.params);
        assert!(head.params[0].ty_span.is_some());
        assert!(head.params[1].ty_span.is_some());
    }

    #[test]
    fn val_spec_extracts_tuple_params() {
        // (int, bool) -> string  → params = [int, bool]
        let (root, _) = parse_text("val add : (int, bool) -> string\n");
        let parsed = parsed_file_from_cst(&root, "val add : (int, bool) -> string\n");
        assert_eq!(parsed.callable_heads.len(), 1);
        let head = &parsed.callable_heads[0];
        assert_eq!(head.params.len(), 2, "expected 2 params, got {:?}", head.params);
    }

    #[test]
    fn val_spec_single_param() {
        // int -> int  → params = [int]
        let (root, _) = parse_text("val f : int -> int\n");
        let parsed = parsed_file_from_cst(&root, "val f : int -> int\n");
        let head = &parsed.callable_heads[0];
        assert_eq!(head.params.len(), 1);
    }

    #[test]
    fn val_spec_no_arrow() {
        // unit → no params (0-arg function)
        let (root, _) = parse_text("val x : int\n");
        let parsed = parsed_file_from_cst(&root, "val x : int\n");
        if let Some(head) = parsed.callable_heads.first() {
            assert_eq!(head.params.len(), 0);
        }
    }

    #[test]
    fn cst_val_spec_produces_decl() {
        let (root, _) = parse_text("val foo : int -> int\n");
        let parsed = parsed_file_from_cst(&root, "val foo : int -> int\n");
        assert_eq!(parsed.decls.len(), 1);
        assert_eq!(parsed.decls[0].name, "foo");
        assert_eq!(parsed.decls[0].kind, DeclKind::Value);
        assert_eq!(parsed.decls[0].role, DeclRole::Declaration);
    }

    #[test]
    #[allow(clippy::print_stderr)]
    fn cst_enum_member_pattern_scope() {
        let input = "enum instr = VI_ADD | VI_SUB\nfunction f(x) = match x {\n  VI_ADD => 1,\n  VI_SUB => 2\n}\n";
        let (root, _) = parse_text(input);
        let parsed = parsed_file_from_cst(&root, input);
        // The pattern VI_ADD should be recorded as TopLevel (enum member), not Local
        let vi_add_occs: Vec<_> =
            parsed.symbol_occurrences.iter().filter(|o| o.name == "VI_ADD").collect();
        eprintln!(
            "VI_ADD occurrences: {:?}",
            vi_add_occs.iter().map(|o| (&o.scope, &o.role, o.span)).collect::<Vec<_>>()
        );
        let pattern_occ = vi_add_occs.iter().find(|o| o.span.start > 30); // in match arm, not enum def
        assert!(pattern_occ.is_some(), "expected VI_ADD occurrence in match arm");
        assert_eq!(
            pattern_occ.unwrap().scope,
            Some(Scope::TopLevel),
            "enum member in pattern should be TopLevel"
        );
    }

    /// J3-4: Enum member doc comments are extracted.
    #[test]
    fn cst_enum_member_doc_comment() {
        let input = "enum Foo = {\n  /// Doc for A\n  A,\n  /// Doc for B\n  B,\n  C\n}\n";
        let (root, _) = parse_text(input);
        let parsed = parsed_file_from_cst(&root, input);
        let members: Vec<_> =
            parsed.decls.iter().filter(|d| d.kind == DeclKind::EnumMember).collect();
        assert_eq!(members.len(), 3, "should have 3 enum members");
        assert_eq!(members[0].name, "A");
        assert_eq!(members[0].doc.as_deref(), Some("Doc for A"));
        assert_eq!(members[1].name, "B");
        assert_eq!(members[1].doc.as_deref(), Some("Doc for B"));
        assert_eq!(members[2].name, "C");
        assert_eq!(members[2].doc, None, "C has no doc");
    }

    #[test]
    fn cst_let_underscore_call_site() {
        let input = "let _ = foo(1, 2)\n";
        let (root, _) = parse_text(input);
        let parsed = parsed_file_from_cst(&root, input);
        let call_names: Vec<_> = parsed.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(call_names.contains(&"foo"), "expected call 'foo', got {:?}", call_names);
    }

    #[test]
    fn cst_typed_binding_from_let() {
        let input = "let x : child = y\n";
        let (root, _) = parse_text(input);
        let parsed = parsed_file_from_cst(&root, input);
        let binding = parsed.typed_bindings.iter().find(|b| b.name == "x");
        assert!(binding.is_some(), "expected typed binding for 'x'");
    }

    #[test]
    fn cst_typed_binding_from_function_params() {
        let input = "function f(x : bits(32), y : int) = x\n";
        let (root, _) = parse_text(input);
        let parsed = parsed_file_from_cst(&root, input);
        let names: Vec<_> = parsed.typed_bindings.iter().map(|b| b.name.as_str()).collect();
        assert!(names.contains(&"x"), "expected typed binding for 'x', got {:?}", names);
        assert!(names.contains(&"y"), "expected typed binding for 'y', got {:?}", names);
    }

    #[test]
    fn cst_function_def_produces_decl_and_head() {
        let (root, _) = parse_text("function add(x, y) = x + y\n");
        let parsed = parsed_file_from_cst(&root, "function add(x, y) = x + y\n");
        assert_eq!(parsed.decls.len(), 1);
        assert_eq!(parsed.decls[0].name, "add");
        assert_eq!(parsed.decls[0].kind, DeclKind::Function);
        assert_eq!(parsed.decls[0].role, DeclRole::Definition);
        assert_eq!(parsed.callable_heads.len(), 1);
        assert_eq!(parsed.callable_heads[0].name, "add");
    }

    #[test]
    fn cst_enum_produces_members() {
        let (root, _) = parse_text("enum color = { Red, Green, Blue }\n");
        let parsed = parsed_file_from_cst(&root, "enum color = { Red, Green, Blue }\n");
        // enum def + 3 members = 4 decls
        let members: Vec<_> =
            parsed.decls.iter().filter(|d| d.kind == DeclKind::EnumMember).collect();
        assert_eq!(members.len(), 3);
        let names: Vec<_> = members.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"Red"));
        assert!(names.contains(&"Green"));
        assert!(names.contains(&"Blue"));
    }

    #[test]
    fn cst_multiple_defs() {
        let input = "val x : int\nfunction f() = 42\ntype myint = int\n";
        let (root, _) = parse_text(input);
        let parsed = parsed_file_from_cst(&root, input);
        assert!(parsed.decls.len() >= 3);
    }

    #[test]
    fn cst_struct_def() {
        let (root, _) = parse_text("struct S = { x : int }\n");
        let parsed = parsed_file_from_cst(&root, "struct S = { x : int }\n");
        assert_eq!(parsed.decls.len(), 1);
        assert_eq!(parsed.decls[0].kind, DeclKind::Struct);
    }

    #[test]
    fn cst_symbol_occurrences() {
        let input = "function f(x) = x + 1\n";
        let (root, _) = parse_text(input);
        let parsed = parsed_file_from_cst(&root, input);
        assert!(!parsed.symbol_occurrences.is_empty());
        let names: Vec<_> = parsed.symbol_occurrences.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"x"));
    }

    #[test]
    fn cst_callable_params() {
        let input = "function add(x, y) = x + y\n";
        let (root, _) = parse_text(input);
        let parsed = parsed_file_from_cst(&root, input);
        assert_eq!(parsed.callable_heads.len(), 1);
        assert!(parsed.callable_heads[0].params.len() >= 2);
    }

    #[test]
    fn cst_call_sites_extracted() {
        let input = "function f() = add(1, 2)\n";
        let (root, _) = parse_text(input);
        let parsed = parsed_file_from_cst(&root, input);
        assert!(!parsed.call_sites.is_empty(), "expected call sites");
        assert_eq!(parsed.call_sites[0].callee, "add");
        assert_eq!(parsed.call_sites[0].caller, Some("f".to_string()));
    }

    #[test]
    fn cst_call_sites_with_args() {
        let input = "function f(x) = g(x, h(1))\n";
        let (root, _) = parse_text(input);
        let parsed = parsed_file_from_cst(&root, input);
        let callees: Vec<_> = parsed.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"g"), "expected call to g");
        assert!(callees.contains(&"h"), "expected call to h");
    }

    #[test]
    fn cst_let_binding_has_local_scope() {
        let input = "function f() = { let x = 1; x + 2 }\n";
        let (root, _) = parse_text(input);
        let parsed = parsed_file_from_cst(&root, input);
        // x should appear with Local scope (definition + reference)
        let x_occs: Vec<_> = parsed
            .symbol_occurrences
            .iter()
            .filter(|s| s.name == "x" && s.scope == Some(Scope::Local))
            .collect();
        assert!(!x_occs.is_empty(), "expected 'x' with Local scope");
    }

    #[test]
    fn cst_let_binding_has_target_span() {
        let input = "function f() = { let y = 1; y + 2 }\n";
        let (root, _) = parse_text(input);
        let parsed = parsed_file_from_cst(&root, input);
        // The reference to 'y' should have target_span pointing to the definition
        let y_refs: Vec<_> = parsed
            .symbol_occurrences
            .iter()
            .filter(|s| s.name == "y" && s.target_span.is_some())
            .collect();
        assert!(!y_refs.is_empty(), "expected 'y' references with target_span");
    }

    #[test]
    fn cst_match_arm_scoped() {
        let input = "function f(x) = match x { Some(v) => v, _ => 0 }\n";
        let (root, _) = parse_text(input);
        let parsed = parsed_file_from_cst(&root, input);
        // 'v' should appear with Local scope
        let v_occs: Vec<_> = parsed
            .symbol_occurrences
            .iter()
            .filter(|s| s.name == "v" && s.scope == Some(Scope::Local))
            .collect();
        assert!(!v_occs.is_empty(), "expected 'v' with Local scope in match arm");
    }

    #[test]
    fn cst_typed_binding_in_let() {
        let input = "function f() = { let x : int = 1; x }\n";
        let (root, _) = parse_text(input);
        let parsed = parsed_file_from_cst(&root, input);
        let typed: Vec<_> = parsed.typed_bindings.iter().filter(|b| b.name == "x").collect();
        assert!(!typed.is_empty(), "expected typed binding for 'x'");
    }
}
