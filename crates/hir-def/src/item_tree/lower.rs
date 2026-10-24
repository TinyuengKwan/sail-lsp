//! CST → ItemTree lowering.
//! Walks the rowan CST and builds the ItemTree with typed arenas.
//! Expression bodies are excluded from signatures so body-only edits
//! don't change the signature_hash.

use super::*;
use crate::type_ref::type_ref_from_text;
use syntax::ast::{traits::HasVisibility, AstNode};

impl ItemTree {
    /// Build an `ItemTree` from a rowan CST root node.
    ///
    /// This is the CST-native path . It walks the typed AST
    /// wrappers produced by the event-based parser and extracts
    /// name + kind + signature for each definition into typed arenas.
    ///
    /// Expression bodies are excluded from the signature (everything
    /// after `=` is stripped) so body-only edits don't change the hash.
    pub fn build_from_cst(root: &syntax::SyntaxNode) -> Self {
        let mut tree = Self::empty();
        let mut fixities = Vec::new();
        for child in root.children() {
            // Extract fixity declarations (typed cast)
            if syntax::ast::FixityDef::can_cast(child.kind()) {
                if let Some(fixity) = extract_fixity_decl(&child) {
                    fixities.push(fixity);
                }
            }
            // Collect $include directive spans (typed cast)
            if syntax::ast::DirectiveDef::can_cast(child.kind()) {
                let text = child.text().to_string();
                let trimmed = text.trim();
                if trimmed.starts_with("$include") {
                    let path = trimmed
                        .strip_prefix("$include")
                        .unwrap_or("")
                        .trim()
                        .trim_matches('"')
                        .trim_matches('<')
                        .trim_matches('>')
                        .to_string();
                    let range = child.text_range();
                    tree.include_spans.push((
                        path,
                        crate::Span::new(usize::from(range.start()), usize::from(range.end())),
                    ));
                }
            }
            if let Some(mod_item) = lower_cst_node(&mut tree, &child) {
                tree.top_level.push(mod_item);
            }
        }
        generated::expand(root, &mut tree, false);
        tree.compute_per_item_hashes();
        tree.fixities = fixities;
        tree
    }

    /// Build the preprocessed declaration surface, including generated names.
    pub fn build_from_cst_full(
        root: &syntax::SyntaxNode,
        symbols: &mut std::collections::HashSet<String>,
    ) -> Self {
        Self::build_from_cst_with_preprocess(root, symbols)
    }

    /// Build from CST with `$ifdef/$ifndef/$define/$iftarget` preprocessing.
    /// Evaluates conditional directives and drops items inside
    /// non-taken branches at ItemTree
    /// construction time.
    ///
    /// `$include` is NOT handled here — files with includes should
    /// use the core_ast path (`ItemTree::build`) which has full
    /// include support via `preprocess.rs`.
    ///
    /// `target`: optional compilation target for `$iftarget`.
    /// `None` = LSP mode (skip `$iftarget` blocks — take else branch).
    pub fn build_from_cst_with_preprocess(
        root: &syntax::SyntaxNode,
        symbols: &mut std::collections::HashSet<String>,
    ) -> Self {
        Self::build_from_cst_with_preprocess_and_target(root, symbols, None)
    }

    /// Build from CST with full preprocessing including `$iftarget`.
    ///
    /// - `target = Some("c")`: `$iftarget c` is taken, else skipped
    /// - `target = None`: LSP mode, `$iftarget` always takes else branch
    pub fn build_from_cst_with_preprocess_and_target(
        root: &syntax::SyntaxNode,
        symbols: &mut std::collections::HashSet<String>,
        target: Option<&str>,
    ) -> Self {
        let mut tree = Self::empty();
        let mut conditions = syntax::preprocess::CondFilter::new();
        for child in root.children() {
            if !conditions.accept(&child, symbols, target) {
                continue;
            }

            if let Some(mod_item) = lower_cst_node(&mut tree, &child) {
                tree.top_level.push(mod_item);
            }
        }

        // Collect fixity declarations (second pass — simple since
        // they're top-level and don't depend on preprocessing)
        let mut fixities = Vec::new();
        for child in root.children() {
            if syntax::ast::FixityDef::can_cast(child.kind()) {
                if let Some(fixity) = extract_fixity_decl(&child) {
                    fixities.push(fixity);
                }
            }
        }

        generated::expand(root, &mut tree, true);
        tree.compute_per_item_hashes();
        tree.fixities = fixities;
        tree
    }
}

/// Allocate a top-level item directly into `tree`'s typed arenas from
/// a CST definition node. Returns the `DefItem` on success.
fn lower_cst_node(tree: &mut ItemTree, node: &syntax::SyntaxNode) -> Option<DefItem> {
    use parser::SyntaxKind as SK;

    let kind = match node.kind() {
        SK::CALLABLE_DEF => None,
        SK::CALLABLE_SPEC => None,
        SK::TYPE_ALIAS_DEF => Some(DefKind::TypeAlias),
        SK::NAMED_DEF => None,
        SK::SCATTERED_DEF => Some(DefKind::ScatteredHead),
        SK::SCATTERED_CLAUSE_DEF => Some(DefKind::ScatteredClause),
        SK::CONSTRAINT_DEF => Some(DefKind::Constraint),
        SK::TERMINATION_MEASURE_DEF => Some(DefKind::TerminationMeasure),
        SK::END_DEF => Some(DefKind::EndMarker),
        SK::INSTANTIATION_DEF => Some(DefKind::Instantiation),
        SK::OUTCOME_DEF => Some(DefKind::Outcome),
        SK::DEFAULT_DEF | SK::FIXITY_DEF | SK::DIRECTIVE_DEF | SK::DEFINITION => return None,
        _ => return None,
    };

    // Walk descendant tokens for keywords, identifiers.
    let mut first_keyword: Option<(SK, String)> = None;
    let mut has_clause_kw = false;
    let mut first_ident: Option<String> = None;
    let mut second_ident: Option<String> = None;
    // Where that second identifier is. `enum clause extension = Ext_S`
    // declares `Ext_S`, and navigation has to be able to land on it.
    let mut second_ident_span: Option<crate::Span> = None;

    for el in node.descendants_with_tokens() {
        // Skip ATTRIBUTE subtrees entirely — they contain $[private]
        // tokens that would be misidentified as the definition keyword.
        if let rowan::NodeOrToken::Node(ref n) = el {
            if syntax::ast::Attribute::can_cast(n.kind()) {
                continue;
            }
        }
        let tok = match el.into_token() {
            Some(t) => t,
            None => continue,
        };
        let tk = tok.kind();
        if tk.is_trivia() {
            continue;
        }
        // Skip attribute punctuation and visibility keywords that appear
        // before the definition keyword. `private struct Foo` should
        // classify as Struct, not Function.
        if matches!(tk, SK::DOLLAR | SK::L_BRACK | SK::R_BRACK | SK::KW_PRIVATE) {
            continue;
        }
        if first_keyword.is_none() && !matches!(tk, SK::IDENT | SK::TY_VAR) {
            first_keyword = Some((tk, tok.text().to_string()));
            continue;
        }
        if first_keyword.is_some() && tk == SK::KW_CLAUSE && !has_clause_kw {
            has_clause_kw = true;
            continue;
        }
        // `let 'cap_size = 32` names a value. `atomic_pat: | kid` is
        // `P_var (P_id (id_of_kid kid), ATyp_var kid)` (parser.mly:541-543)
        // and `id_of_kid` drops the leading `'` (ast_util.ml:983), so the
        // definition binds `cap_size` — and constrains it to `atom('cap_size)`.
        // Only `SK::IDENT` counted as a name here, so the whole definition was
        // dropped from the item tree and every mention of it was unresolved.
        let names_a_value = tk == SK::IDENT
            || (tk == SK::TY_VAR
                && matches!(
                    first_keyword.as_ref().map(|(k, _)| *k),
                    Some(SK::KW_LET) | Some(SK::KW_VAR)
                ));
        if names_a_value {
            if first_ident.is_none() {
                first_ident = Some(tok.text().trim_start_matches('\'').to_string());
            } else if second_ident.is_none() {
                second_ident = Some(tok.text().to_string());
                let r = tok.text_range();
                second_ident_span =
                    Some(crate::Span::new(usize::from(r.start()), usize::from(r.end())));
                break;
            }
        }
        if first_ident.is_some() && !has_clause_kw {
            break;
        }
    }

    let name_str = first_ident.unwrap_or_default();
    if name_str.is_empty() && kind != Some(DefKind::Constraint) && kind != Some(DefKind::EndMarker)
    {
        return None;
    }

    let def_kind = kind.unwrap_or_else(|| match first_keyword.as_ref().map(|(k, _)| *k) {
        Some(SK::KW_FUNCTION) => DefKind::Function,
        Some(SK::KW_VAL) => DefKind::ValSpec,
        Some(SK::KW_MAPPING) => DefKind::Mapping,
        Some(SK::KW_TYPE) => DefKind::TypeAlias,
        Some(SK::KW_STRUCT) => DefKind::Struct,
        Some(SK::KW_ENUM) => DefKind::Enum,
        Some(SK::KW_UNION) => DefKind::Union,
        Some(SK::KW_BITFIELD) => DefKind::Bitfield,
        Some(SK::KW_NEWTYPE) => DefKind::Newtype,
        Some(SK::KW_REGISTER) => DefKind::Register,
        Some(SK::KW_LET) => DefKind::Let,
        Some(SK::KW_VAR) => DefKind::Var,
        Some(SK::KW_OVERLOAD) => DefKind::Overload,
        Some(SK::KW_SCATTERED) => DefKind::ScatteredHead,
        Some(SK::KW_CONSTRAINT) => DefKind::Constraint,
        Some(SK::KW_TERMINATION_MEASURE) => DefKind::TerminationMeasure,
        _ => DefKind::Function,
    });

    // The member a clause contributes, and only for the clause forms that
    // contribute one. `enum clause E = M` and `union clause U = C : T` name a
    // member; `function clause f(a, b)` and `mapping clause m = ..` name a
    // parameter, and treating that as a member registers it as an enum
    // member, a constructor and a pattern constant across the whole
    // workspace — which is how `priv` in
    // `function clause is_CSR_accessible(0x180, priv, _)` stopped being a
    // binding everywhere.
    let clause_declares_member =
        matches!(first_keyword.as_ref().map(|(k, _)| *k), Some(SK::KW_ENUM) | Some(SK::KW_UNION));
    let member_name = if has_clause_kw
        && clause_declares_member
        && matches!(def_kind, DefKind::Union | DefKind::Enum | DefKind::ScatteredClause)
    {
        second_ident
    } else {
        None
    };

    let full_text = node.text().to_string();
    let signature_text = extract_signature(&full_text);
    // For types with braced member lists (enum/union/struct/bitfield),
    // use the full text so extract_braced_idents_from_sig can find members.
    let body_sig = match def_kind {
        DefKind::Enum | DefKind::Union | DefKind::Struct | DefKind::Bitfield => full_text.clone(),
        // A `type X = Y` or `newtype X = Y` is *only* interesting for what
        // is right of the `=`, and `extract_signature` cuts exactly there —
        // so the item tree recorded `type X` and nothing knew what X aliased.
        DefKind::TypeAlias | DefKind::Newtype => full_text.clone(),
        _ => signature_text.clone(),
    };
    let range = node.text_range();
    let span = crate::Span::new(usize::from(range.start()), usize::from(range.end()));
    // Where a cursor should land when navigating here. Falls back to the
    // whole definition when the name is not a plain identifier — `val
    // "shift_bits_right"` and `overload operator +` both reach this.
    let name_span = syntax::definition_name_span(node).unwrap_or(span);
    let doc = extract_doc_comment(node);
    let visibility = extract_visibility(node);
    let name = Name::from(name_str.as_str());

    // Allocate directly into the typed arena.
    let mod_item = match def_kind {
        DefKind::Function => {
            let type_ref = Some(type_ref_from_text(&signature_text));
            let id = tree.functions.alloc(Function {
                name,
                signature: signature_text,
                type_ref,
                span,
                name_span,
                is_clause: has_clause_kw,
                member_name,
                doc,
                visibility,
                signature_hash: 0,
            });
            DefItem::Function(id)
        }
        DefKind::Struct => {
            let type_ref = Some(type_ref_from_text(&signature_text));
            let id = tree.type_defs.alloc(TypeDef {
                name,
                kind: TypeDefKind::Struct,
                signature: body_sig,
                type_ref,
                span,
                name_span,
                fields: extract_field_decls(node),
                is_clause: has_clause_kw,
                member_name,
                doc,
                visibility,
                signature_hash: 0,
            });
            DefItem::TypeDef(id)
        }
        DefKind::Union => {
            let type_ref = Some(type_ref_from_text(&signature_text));
            let id = tree.type_defs.alloc(TypeDef {
                name,
                kind: TypeDefKind::Union,
                signature: body_sig,
                type_ref,
                span,
                name_span,
                fields: extract_field_decls(node),
                is_clause: has_clause_kw,
                member_name,
                doc,
                visibility,
                signature_hash: 0,
            });
            DefItem::TypeDef(id)
        }
        DefKind::Enum => {
            let type_ref = Some(type_ref_from_text(&signature_text));
            let id = tree.type_defs.alloc(TypeDef {
                name,
                kind: TypeDefKind::Enum,
                signature: body_sig,
                type_ref,
                span,
                name_span,
                fields: extract_field_decls(node),
                is_clause: has_clause_kw,
                member_name,
                doc,
                visibility,
                signature_hash: 0,
            });
            DefItem::TypeDef(id)
        }
        DefKind::Bitfield => {
            let type_ref = Some(type_ref_from_text(&signature_text));
            let id = tree.type_defs.alloc(TypeDef {
                name,
                kind: TypeDefKind::Bitfield,
                signature: body_sig,
                type_ref,
                span,
                name_span,
                fields: extract_field_decls(node),
                is_clause: has_clause_kw,
                member_name,
                doc,
                visibility,
                signature_hash: 0,
            });
            DefItem::TypeDef(id)
        }
        DefKind::Newtype => {
            let type_ref = Some(type_ref_from_text(&body_sig));
            let id = tree.type_defs.alloc(TypeDef {
                name,
                kind: TypeDefKind::Newtype,
                signature: body_sig,
                type_ref,
                span,
                name_span,
                // `newtype virtaddr = Virtaddr : xlenbits` declares *two*
                // names — the type and its constructor — the same way
                // `union virtaddr = { Virtaddr : xlenbits }` does, which is
                // what upstream desugars it to. Only the type was recorded,
                // so `Virtaddr`, `Physaddr`, `Regno` and `Regidx` — the
                // wrappers sail-riscv puts on every address and register
                // number — had no definition to navigate to and no member
                // for any feature to ask about.
                fields: extract_field_decls(node),
                is_clause: has_clause_kw,
                member_name,
                doc,
                visibility,
                signature_hash: 0,
            });
            DefItem::TypeDef(id)
        }
        DefKind::TypeAlias => {
            let type_ref = Some(type_ref_from_text(&body_sig));
            let id = tree.type_defs.alloc(TypeDef {
                name,
                kind: TypeDefKind::TypeAlias,
                signature: body_sig,
                type_ref,
                span,
                name_span,
                fields: Vec::new(),
                is_clause: has_clause_kw,
                member_name,
                doc,
                visibility,
                signature_hash: 0,
            });
            DefItem::TypeDef(id)
        }
        DefKind::Register => {
            let type_ref = Some(type_ref_from_text(&signature_text));
            let id = tree.registers.alloc(Register {
                name,
                signature: signature_text,
                type_ref,
                span,
                name_span,
                doc,
                visibility,
            });
            DefItem::Register(id)
        }
        // An `outcome` declares a callable exactly as a `val` does — the
        // implementation comes from an `instantiation` — so it is stored
        // the same way and merges across files the same way. It used to
        // return `None` here, which kept it out of the item tree, out of
        // the workspace merge, and so out of every calling file's scope.
        DefKind::ValSpec | DefKind::Outcome => {
            let type_ref = Some(type_ref_from_text(&signature_text));
            let id = tree.val_specs.alloc(ValSpec {
                name,
                signature: signature_text,
                type_ref,
                span,
                name_span,
                doc,
                visibility,
                signature_hash: 0,
            });
            DefItem::ValSpec(id)
        }
        DefKind::Mapping | DefKind::MappingSpec => {
            let type_ref = Some(type_ref_from_text(&signature_text));
            let id = tree.mappings.alloc(Mapping {
                name,
                signature: signature_text,
                type_ref,
                span,
                name_span,
                is_clause: has_clause_kw,
                doc,
                visibility,
                signature_hash: 0,
            });
            DefItem::Mapping(id)
        }
        DefKind::Let => {
            let type_ref = Some(type_ref_from_text(&signature_text));
            let id = tree.lets.alloc(LetDef {
                name,
                signature: signature_text,
                type_ref,
                span,
                name_span,
                is_var: false,
                doc,
                visibility,
            });
            DefItem::Let(id)
        }
        DefKind::Var => {
            let type_ref = Some(type_ref_from_text(&signature_text));
            let id = tree.lets.alloc(LetDef {
                name,
                signature: signature_text,
                type_ref,
                span,
                name_span,
                is_var: true,
                doc,
                visibility,
            });
            DefItem::Let(id)
        }
        DefKind::Overload => {
            let id = tree.overloads.alloc(Overload {
                name,
                signature: signature_text,
                type_ref: None,
                span,
                name_span,
                visibility,
            });
            DefItem::Overload(id)
        }
        DefKind::ScatteredHead => {
            let id = tree.scattered.alloc(ScatteredDef {
                fields: Vec::new(),
                type_kind: scattered_type_kind(node),
                member_span: second_ident_span,
                name,
                signature: signature_text,
                type_ref: None,
                span,
                name_span,
                is_head: true,
                member_name: None,
                doc,
                visibility,
            });
            DefItem::Scattered(id)
        }
        DefKind::ScatteredClause => {
            // Extract payload type from the FULL text (after `=`) for union clauses.
            // signature_text only has the part BEFORE `=` (e.g., "union clause instruction").
            // The constructor and type are AFTER `=`:
            //   "union clause instruction = FVVTYPE : (fvvfunct6, bits(1), vregidx, ...)"
            // We need the part after `:` in the full text.
            let type_ref = full_text
                .find('=')
                .and_then(|eq_pos| full_text[eq_pos + 1..].find(':').map(|c| eq_pos + 1 + c))
                .map(|colon_pos| {
                    let type_text = full_text[colon_pos + 1..].trim();
                    crate::hir::type_ref::type_ref_from_text(type_text)
                });
            let id = tree.scattered.alloc(ScatteredDef {
                fields: extract_field_decls(node),
                type_kind: scattered_type_kind(node),
                member_span: second_ident_span,
                name,
                signature: signature_text,
                type_ref,
                span,
                name_span,
                is_head: false,
                member_name,
                doc,
                visibility,
            });
            DefItem::Scattered(id)
        }
        DefKind::Constraint => {
            let id = tree.constraints.alloc(Constraint {
                name,
                signature: signature_text,
                type_ref: None,
                span,
                name_span,
                visibility,
            });
            DefItem::Constraint(id)
        }
        DefKind::TerminationMeasure => {
            let id = tree.pragmas.alloc(Pragma {
                name,
                text: signature_text,
                span,
                name_span,
                pragma_kind: PragmaKind::TerminationMeasure,
                visibility,
            });
            DefItem::Pragma(id)
        }
        DefKind::EndMarker => {
            let id = tree.pragmas.alloc(Pragma {
                name,
                text: signature_text,
                span,
                name_span,
                pragma_kind: PragmaKind::EndMarker,
                visibility,
            });
            DefItem::Pragma(id)
        }
        DefKind::Instantiation => {
            let id = tree.pragmas.alloc(Pragma {
                name,
                text: signature_text,
                span,
                name_span,
                pragma_kind: PragmaKind::Instantiation,
                visibility,
            });
            DefItem::Pragma(id)
        }
        // A field or variant is a member of a type definition, not something
        // the top-level walk can arrive at. Both reach the symbol index
        // through `DefItem::fields`.
        DefKind::Field | DefKind::Variant => return None,
    };
    Some(mod_item)
}

fn scattered_type_kind(node: &syntax::SyntaxNode) -> Option<TypeDefKind> {
    node.children_with_tokens().find_map(|token| match token.kind() {
        parser::SyntaxKind::KW_UNION => Some(TypeDefKind::Union),
        parser::SyntaxKind::KW_ENUM => Some(TypeDefKind::Enum),
        _ => None,
    })
}

/// Extract visibility from `@private` attribute on a CST node.
///
/// Uses the typed `HasVisibility` trait when the node can be cast to a
/// definition type (CallableDef, NamedDef, etc.). Falls back to manual
/// sibling and text inspection for untyped nodes.
fn extract_visibility(node: &syntax::SyntaxNode) -> crate::visibility::RawVisibility {
    use parser::SyntaxKind as SK;

    // Try typed HasVisibility accessor via cast to known definition types.
    // Each of these types has a generated `impl HasVisibility` and a
    // `visibility()` accessor that checks for a Visibility child node.
    macro_rules! try_vis {
        ($($ty:ident),+ $(,)?) => {
            $(
                if let Some(def) = syntax::ast::$ty::cast(node.clone()) {
                    if def.is_private() {
                        return crate::visibility::RawVisibility::Private;
                    }
                }
            )+
        };
    }
    try_vis!(
        CallableDef,
        CallableSpec,
        NamedDef,
        ScatteredDef,
        ScatteredClauseDef,
        TypeAliasDef,
        OutcomeDef,
    );

    // Fallback: Check the node's own text for embedded `$[private]` or `@private`
    let text = node.text().to_string();
    let trimmed = text.trim_start();
    if trimmed.starts_with("$[private]") || trimmed.starts_with("@private") {
        return crate::visibility::RawVisibility::Private;
    }

    // Fallback: Walk preceding siblings for `$[private]` attribute pattern
    let mut sibling = node.prev_sibling_or_token();
    while let Some(el) = sibling {
        match el.kind() {
            SK::WHITESPACE | SK::DOC_COMMENT => {}
            SK::DEFINITION => {
                let def_text = match &el {
                    rowan::NodeOrToken::Node(n) => n.text().to_string(),
                    rowan::NodeOrToken::Token(t) => t.text().to_string(),
                };
                if def_text.contains("private") {
                    return crate::visibility::RawVisibility::Private;
                }
                break;
            }
            SK::ATTRIBUTE => {
                let attr_text = match &el {
                    rowan::NodeOrToken::Node(n) => n.text().to_string(),
                    rowan::NodeOrToken::Token(t) => t.text().to_string(),
                };
                if attr_text.contains("private") {
                    return crate::visibility::RawVisibility::Private;
                }
            }
            SK::L_BRACK | SK::DOLLAR | SK::R_BRACK => {}
            _ => break,
        }
        sibling = el.prev_sibling_or_token();
    }

    // Fallback: Check child tokens for `KW_PRIVATE` keyword
    for tok in node.descendants_with_tokens().filter_map(|el| el.into_token()) {
        let tk = tok.kind();
        if tk == SK::KW_PRIVATE {
            return crate::visibility::RawVisibility::Private;
        }
        if tk == SK::IDENT {
            break;
        }
    }

    crate::visibility::RawVisibility::Public
}

// first_ident_in_descendants removed: callers now use typed HasName
// accessors directly (e.g., `named_def.name_text()`).

/// Extract `///` doc comments from the CST siblings preceding a definition node.
///
/// Walks backward from `node`, skipping whitespace, collecting consecutive
/// DOC_COMMENT tokens. Returns `None` if no doc comments are found.
fn extract_doc_comment(node: &syntax::SyntaxNode) -> Option<String> {
    use parser::SyntaxKind as SK;

    let mut doc_lines: Vec<String> = Vec::new();
    let mut sibling = node.first_token().and_then(|token| token.prev_token());

    while let Some(el) = sibling {
        match el.kind() {
            SK::WHITESPACE => {
                // Skip whitespace between doc comment lines
            }
            SK::DOC_COMMENT => {
                let text = el.text().to_string();
                // Strip `/// ` or `///` prefix
                let stripped =
                    text.strip_prefix("/// ").or_else(|| text.strip_prefix("///")).unwrap_or(&text);
                doc_lines.push(stripped.to_string());
            }
            // `//` is not documentation. Upstream's lexer lexes `///` as
            // `DocLine` and `/*!` as `DocBlock`, and throws `//` into a
            // comment list the AST never sees (lexer.mll:228-239). Hover
            // used to show plain `//` above a definition, because the text
            // scanner it used could not tell them apart.
            _ => break,
        }
        sibling = el.prev_token();
    }

    if doc_lines.is_empty() {
        return None;
    }
    // Reverse because we collected backward
    doc_lines.reverse();
    Some(doc_lines.join("\n"))
}

/// Extract a fixity declaration from a FIXITY_DEF CST node.
/// Input: `infixl 6 op_name` or `infix 4 ~~`
fn extract_fixity_decl(node: &syntax::SyntaxNode) -> Option<FixityDecl> {
    use parser::SyntaxKind as SK;

    let mut assoc = Associativity::Left;
    let mut level: Option<u8> = None;
    let mut operator: Option<String> = None;

    for tok in node.descendants_with_tokens().filter_map(|el| el.into_token()) {
        let kind = tok.kind();
        if kind.is_trivia() {
            continue;
        }
        match kind {
            SK::KW_INFIX => assoc = Associativity::None,
            SK::KW_INFIXL => assoc = Associativity::Left,
            SK::KW_INFIXR => assoc = Associativity::Right,
            SK::NUM_LIT => {
                level = tok.text().parse::<u8>().ok();
            }
            SK::IDENT if operator.is_none() => {
                operator = Some(tok.text().to_string());
            }
            // Operator symbols like `+`, `*`, `~`, `@` etc.
            _ if operator.is_none()
                && !matches!(kind, SK::KW_INFIX | SK::KW_INFIXL | SK::KW_INFIXR | SK::NUM_LIT) =>
            {
                let text = tok.text().to_string();
                if !text.is_empty() {
                    operator = Some(text);
                }
            }
            _ => {}
        }
    }

    Some(FixityDecl { operator: operator?, level: level.unwrap_or(0), assoc })
}

/// Extract field names from a bitfield node's full text.
/// Input format: "bitfield Name : bits(N) = { field1 : hi .. lo, field2 : ... }"
/// Returns: ["field1", "field2"]
/// Field declarations of a `struct` or `bitfield`, with the span of each name.
///
/// The grammar does not give a struct body its own nodes — the whole
/// definition is one `NAMED_DEF` — so this reads the token stream. At brace
/// depth 1 a field name is the first `IDENT` after the opening `{` or after a
/// `,`; everything up to the next `,` at that depth is its type, which covers
/// a struct's `variety : Access_variety` and a bitfield's `SSI : 1` and
/// `X : 0 .. 3` alike.
///
/// Generated bitfield bindings and type inference share these field declarations.
pub fn extract_field_decls(node: &syntax::SyntaxNode) -> Vec<crate::item_tree::FieldDecl> {
    use crate::visibility::RawVisibility;
    use parser::SyntaxKind as SK;
    let mut out: Vec<crate::item_tree::FieldDecl> = Vec::new();
    let mut depth = 0usize;
    // A comma only separates members at the top of the braces. `A : (int,
    // int)` has one inside its type, and treating that as a separator makes
    // the next token — `int` — look like the next member's name.
    let mut group = 0usize;
    let mut expect_name = false;
    let mut type_start: Option<usize> = None;
    let mut visibility = RawVisibility::Public;

    let finish_type = |out: &mut Vec<crate::item_tree::FieldDecl>,
                       type_start: &mut Option<usize>,
                       end: usize,
                       text: &str| {
        if let (Some(start), Some(last)) = (type_start.take(), out.last_mut()) {
            if start <= end && end <= text.len() {
                last.type_text = text[start..end].trim().to_string();
            }
        }
    };

    let source = node.text().to_string();
    let base = usize::from(node.text_range().start());
    // `enum E = A | B` — Sail's bar-separated form, with no braces at all.
    // The loop below opens on `{`, so it returned nothing for these, and
    // every feature that asks an enum for its members got an empty list.
    if !source.contains('{') {
        let mut expect = false;
        // `newtype regno = Regno : int` — the constructor carries a payload
        // where an `enum E = A | B` member does not. Recording it empty made
        // hover say `regno::Regno` and stop.
        let mut payload_start: Option<usize> = None;
        for tok in node.descendants_with_tokens().filter_map(|el| el.into_token()) {
            let (start, end) =
                (usize::from(tok.text_range().start()), usize::from(tok.text_range().end()));
            match tok.kind() {
                SK::EQ | SK::PIPE => {
                    finish_type(&mut out, &mut payload_start, start - base, &source);
                    expect = true;
                    visibility = RawVisibility::Public;
                }
                SK::KW_PRIVATE if expect => visibility = RawVisibility::Private,
                SK::IDENT if expect => {
                    out.push(crate::item_tree::FieldDecl {
                        name: Name::from(tok.text()),
                        visibility,
                        name_span: crate::Span::new(start, end),
                        type_text: String::new(),
                    });
                    expect = false;
                }
                SK::COLON if !out.is_empty() && payload_start.is_none() => {
                    payload_start = Some(end - base);
                }
                k if k.is_trivia() => {}
                _ => expect = false,
            }
        }
        finish_type(&mut out, &mut payload_start, source.len(), &source);
        return out;
    }
    for tok in node.descendants_with_tokens().filter_map(|el| el.into_token()) {
        let start = usize::from(tok.text_range().start());
        let end = usize::from(tok.text_range().end());
        match tok.kind() {
            SK::L_CURLY => {
                depth += 1;
                if depth == 1 {
                    expect_name = true;
                    visibility = RawVisibility::Public;
                }
            }
            SK::R_CURLY => {
                if depth == 1 {
                    finish_type(&mut out, &mut type_start, start - base, &source);
                    expect_name = false;
                }
                depth = depth.saturating_sub(1);
            }
            SK::L_PAREN | SK::L_BRACK => group += 1,
            SK::R_PAREN | SK::R_BRACK => group = group.saturating_sub(1),
            SK::COMMA if depth == 1 && group == 0 => {
                finish_type(&mut out, &mut type_start, start - base, &source);
                expect_name = true;
                visibility = RawVisibility::Public;
            }
            SK::KW_PRIVATE if depth == 1 && group == 0 && expect_name => {
                visibility = RawVisibility::Private;
            }
            SK::COLON if depth == 1 && group == 0 && !out.is_empty() && type_start.is_none() => {
                type_start = Some(end - base);
            }
            SK::IDENT if depth == 1 && group == 0 && expect_name => {
                out.push(crate::item_tree::FieldDecl {
                    name: Name::from(tok.text()),
                    visibility,
                    name_span: crate::Span::new(start, end),
                    type_text: String::new(),
                });
                expect_name = false;
            }
            _ => {}
        }
    }
    out
}

fn extract_signature(text: &str) -> String {
    // Find the first `=` that's not part of `==`, `!=`, `=>`, `<=`, `>=`, `<->`
    let bytes = text.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'=' {
            // Check it's not ==, =>, !=, <=, >=
            let prev = if i > 0 { bytes[i - 1] } else { 0 };
            let next = if i + 1 < bytes.len() { bytes[i + 1] } else { 0 };
            if prev == b'!' || prev == b'<' || prev == b'>' {
                continue;
            }
            if next == b'=' || next == b'>' {
                continue;
            }
            // Found a standalone `=`
            return text[..i].trim_end().to_string();
        }
    }
    // No `=` found — the entire text is the signature
    text.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn cst_tree(source: &str) -> ItemTree {
        let (root, _) = syntax::parse_text(source);
        ItemTree::build_from_cst(&root)
    }

    #[test]
    fn cst_empty_file() {
        let tree = cst_tree("");
        assert!(tree.is_empty());
    }

    #[test]
    fn cst_val_spec_name_and_kind() {
        let tree = cst_tree("val add : (int, int) -> int\n");
        assert_eq!(tree.len(), 1);
        let items = tree.top_level_items();
        assert_eq!(items[0].name(&tree).as_str(), "add");
        assert_eq!(items[0].def_kind(&tree), DefKind::ValSpec);
    }

    #[test]
    fn cst_function_def_name_and_kind() {
        let tree = cst_tree("function foo(x) = x + 1\n");
        assert_eq!(tree.len(), 1);
        let items = tree.top_level_items();
        assert_eq!(items[0].name(&tree).as_str(), "foo");
        assert_eq!(items[0].def_kind(&tree), DefKind::Function);
    }

    #[test]
    fn cst_multiple_definitions() {
        let tree = cst_tree("val foo : int -> int\nfunction foo(x) = x + 1\n");
        let names: Vec<_> =
            tree.top_level_items().iter().map(|id| id.name(&tree).as_str()).collect();
        assert_eq!(names, vec!["foo", "foo"]);
    }

    #[test]
    fn cst_enum_is_named_def() {
        let tree = cst_tree("enum color = { Red, Green, Blue }\n");
        assert_eq!(tree.len(), 3);
        let items = tree.top_level_items();
        assert_eq!(items[0].name(&tree).as_str(), "color");
        assert_eq!(items[0].def_kind(&tree), DefKind::Enum);
        assert_eq!(items[1].name(&tree).as_str(), "color_of_num");
        assert_eq!(items[2].name(&tree).as_str(), "num_of_color");
        assert!(items[1..].iter().all(|item| tree.generated[item].owner == items[0]));
    }

    #[test]
    fn cst_struct_is_named_def() {
        let tree = cst_tree("struct S = { x : int }\n");
        assert_eq!(tree.len(), 1);
        let items = tree.top_level_items();
        assert_eq!(items[0].name(&tree).as_str(), "S");
        assert_eq!(items[0].def_kind(&tree), DefKind::Struct);
    }

    #[test]
    fn cst_type_alias() {
        let tree = cst_tree("type myint = int\n");
        assert_eq!(tree.len(), 1);
        let items = tree.top_level_items();
        assert_eq!(items[0].name(&tree).as_str(), "myint");
        assert_eq!(items[0].def_kind(&tree), DefKind::TypeAlias);
    }

    #[test]
    fn cst_body_edit_does_not_change_signature() {
        let tree_a = cst_tree("function foo(x) = x + 1\n");
        let tree_b = cst_tree("function foo(x) = x + 2\n");
        // Signature is everything before `=`, so body changes don't matter
        assert_eq!(
            tree_a.top_level_items()[0].signature(&tree_a),
            tree_b.top_level_items()[0].signature(&tree_b)
        );
        assert_eq!(tree_a.signature_hash, tree_b.signature_hash);
    }

    #[test]
    fn cst_signature_change_changes_hash() {
        let tree_a = cst_tree("val foo : int -> int\n");
        let tree_b = cst_tree("val foo : int -> bool\n");
        assert_ne!(tree_a.signature_hash, tree_b.signature_hash);
    }

    #[test]
    fn cst_directives_excluded() {
        let tree = cst_tree("$option --foo\nval x : int\n");
        // Only `val x` should appear
        let names: Vec<_> =
            tree.top_level_items().iter().map(|id| id.name(&tree).as_str()).collect();
        assert!(names.contains(&"x"));
        assert!(!names.iter().any(|n| n.starts_with("$")));
    }

    #[test]
    fn cst_scattered_def() {
        let tree = cst_tree("scattered function foo\n");
        assert_eq!(tree.len(), 1);
        let items = tree.top_level_items();
        assert_eq!(items[0].name(&tree).as_str(), "foo");
        assert_eq!(items[0].def_kind(&tree), DefKind::ScatteredHead);
    }

    #[test]
    fn cst_register_def() {
        let tree = cst_tree("register PC : bits(64)\n");
        assert_eq!(tree.len(), 1);
        let items = tree.top_level_items();
        assert_eq!(items[0].name(&tree).as_str(), "PC");
        assert_eq!(items[0].def_kind(&tree), DefKind::Register);
    }

    #[test]
    fn cst_entry_lookup() {
        let tree = cst_tree("val foo : int\nval bar : bool\n");
        assert!(tree.find_by_name("foo").is_some());
        assert!(tree.find_by_name("bar").is_some());
        assert!(tree.find_by_name("missing").is_none());
    }

    fn cst_tree_preprocess(source: &str) -> ItemTree {
        let (root, _) = syntax::parse_text(source);
        let mut symbols = std::collections::HashSet::new();
        // Add default FEATURE_* symbols like the real preprocessor
        {
            let s = &"FEATURE_UNION_BARRIER";
            symbols.insert(s.to_string());
        }
        ItemTree::build_from_cst_with_preprocess(&root, &mut symbols)
    }

    #[test]
    fn preprocess_ifdef_taken() {
        let tree = cst_tree_preprocess(
            "\
$define MY_FEATURE
$ifdef MY_FEATURE
val foo : int
$endif
val bar : bool
",
        );
        // Both foo (inside taken ifdef) and bar should appear
        assert!(tree.find_by_name("foo").is_some(), "foo should be in taken ifdef");
        assert!(tree.find_by_name("bar").is_some());
    }

    #[test]
    fn preprocess_ifdef_not_taken() {
        let tree = cst_tree_preprocess(
            "\
$ifdef UNDEFINED_FEATURE
val hidden : int
$endif
val visible : bool
",
        );
        // hidden should be filtered out, visible should remain
        assert!(tree.find_by_name("hidden").is_none(), "hidden should be filtered by ifdef");
        assert!(tree.find_by_name("visible").is_some());
    }

    /// `default Order dec` must define `_DEFAULT_ORDER_SET` and `_DEFAULT_DEC`
    /// (upstream preprocess.ml:292-301). Without them the bundled prelude's
    /// `$ifdef _DEFAULT_DEC` blocks silently take the inc-order branch, giving
    /// `subrange_bits`, `bitvector_access` and friends the wrong signatures.
    #[test]
    fn default_order_defines_preprocessor_symbols() {
        use syntax::preprocess::{default_order_of_file, DefaultOrder};

        let source = "\
default Order dec
$ifdef _DEFAULT_DEC
val dec_variant : int
$else
val inc_variant : int
$endif
";
        let (root, _) = syntax::parse_text(source);
        assert_eq!(default_order_of_file(&root), Some(DefaultOrder::Dec));

        let mut symbols = syntax::preprocess::default_symbols();
        DefaultOrder::Dec.apply(&mut symbols);
        assert!(symbols.contains("_DEFAULT_ORDER_SET"));

        let tree = ItemTree::build_from_cst_with_preprocess(&root, &mut symbols);
        assert!(tree.find_by_name("dec_variant").is_some(), "dec branch must be taken");
        assert!(tree.find_by_name("inc_variant").is_none(), "inc branch must be dropped");
    }

    #[test]
    fn default_order_inc_takes_else_branch() {
        use syntax::preprocess::{default_order_of_file, DefaultOrder};

        let source = "\
default Order inc
$ifdef _DEFAULT_DEC
val dec_variant : int
$else
val inc_variant : int
$endif
";
        let (root, _) = syntax::parse_text(source);
        assert_eq!(default_order_of_file(&root), Some(DefaultOrder::Inc));

        let mut symbols = syntax::preprocess::default_symbols();
        DefaultOrder::Inc.apply(&mut symbols);
        let tree = ItemTree::build_from_cst_with_preprocess(&root, &mut symbols);
        assert!(tree.find_by_name("inc_variant").is_some());
        assert!(tree.find_by_name("dec_variant").is_none());
    }

    /// With no `default Order` anywhere, neither symbol is defined — which is
    /// what lets `lib/vector.sail`'s `$ifndef _DEFAULT_ORDER_SET` fire.
    #[test]
    fn no_default_order_leaves_symbols_unset() {
        let (root, _) = syntax::parse_text("val f : int\n");
        assert_eq!(syntax::preprocess::default_order_of_file(&root), None);
        let symbols = syntax::preprocess::default_symbols();
        assert!(!symbols.contains("_DEFAULT_ORDER_SET"));
        assert!(!symbols.contains("_DEFAULT_DEC"));
    }

    #[test]
    fn preprocess_ifndef() {
        let tree = cst_tree_preprocess(
            "\
$ifndef UNDEFINED_FEATURE
val present : int
$endif
",
        );
        assert!(tree.find_by_name("present").is_some(), "ifndef of undefined should take branch");
    }

    #[test]
    fn preprocess_else_branch() {
        let tree = cst_tree_preprocess(
            "\
$ifdef UNDEFINED
val hidden : int
$else
val fallback : bool
$endif
",
        );
        assert!(tree.find_by_name("hidden").is_none());
        assert!(tree.find_by_name("fallback").is_some());
    }

    #[test]
    fn preprocess_define_then_ifdef() {
        let tree = cst_tree_preprocess(
            "\
$define NEW_SYM
$ifdef NEW_SYM
val defined_later : int
$endif
",
        );
        assert!(tree.find_by_name("defined_later").is_some());
    }

    #[test]
    fn preprocess_no_directives() {
        let tree = cst_tree_preprocess("val x : int\nfunction f() = 42\n");
        assert!(tree.find_by_name("x").is_some());
        assert!(tree.find_by_name("f").is_some());
    }

    fn cst_tree_with_target(source: &str, target: Option<&str>) -> ItemTree {
        let (root, _) = syntax::parse_text(source);
        let mut symbols = syntax::preprocess::default_symbols();
        ItemTree::build_from_cst_with_preprocess_and_target(&root, &mut symbols, target)
    }

    #[test]
    fn iftarget_with_matching_target() {
        let tree = cst_tree_with_target(
            "\
$iftarget c
val c_only : int
$endif
val always : bool
",
            Some("c"),
        );
        assert!(tree.find_by_name("c_only").is_some(), "c_only should be in taken iftarget");
        assert!(tree.find_by_name("always").is_some());
    }

    #[test]
    fn iftarget_with_non_matching_target() {
        let tree = cst_tree_with_target(
            "\
$iftarget c
val c_only : int
$endif
val always : bool
",
            Some("ocaml"),
        );
        assert!(tree.find_by_name("c_only").is_none(), "c_only should be skipped for ocaml target");
        assert!(tree.find_by_name("always").is_some());
    }

    #[test]
    fn iftarget_multi_target_set() {
        // $iftarget with multiple targets: "c ocaml"
        let tree = cst_tree_with_target(
            "\
$iftarget c ocaml
val multi : int
$endif
",
            Some("ocaml"),
        );
        assert!(tree.find_by_name("multi").is_some(), "ocaml should match 'c ocaml' target set");
    }

    #[test]
    fn iftarget_none_target_takes_else() {
        // LSP mode: target=None → take else branch
        let tree = cst_tree_with_target(
            "\
$iftarget c
val target_only : int
$else
val fallback : bool
$endif
",
            None,
        );
        assert!(tree.find_by_name("target_only").is_none(), "LSP mode should skip iftarget");
        assert!(tree.find_by_name("fallback").is_some(), "LSP mode should take else branch");
    }

    #[test]
    fn iftarget_with_else_matching() {
        let tree = cst_tree_with_target(
            "\
$iftarget c
val c_impl : int
$else
val default_impl : bool
$endif
",
            Some("c"),
        );
        assert!(tree.find_by_name("c_impl").is_some(), "c_impl should be taken");
        assert!(tree.find_by_name("default_impl").is_none(), "default_impl should be skipped");
    }

    fn cst_tree_full(source: &str) -> ItemTree {
        let (root, _) = syntax::parse_text(source);
        let mut symbols = std::collections::HashSet::new();
        ItemTree::build_from_cst_full(&root, &mut symbols)
    }

    #[test]
    fn cst_bitfield_generates_accessors() {
        let tree = cst_tree_full("bitfield Foo : bits(8) = { lo : 3 .. 0 }\n");
        let names: Vec<_> =
            tree.top_level_items().iter().map(|id| id.name(&tree).as_str()).collect();
        assert!(names.contains(&"Foo"), "original bitfield entry: {names:?}");
        assert!(names.contains(&"Mk_Foo"), "constructor: {names:?}");
        assert!(names.contains(&"_get_Foo_lo"), "getter: {names:?}");
        assert!(names.contains(&"_update_Foo_lo"), "updater: {names:?}");
        assert!(names.contains(&"_set_Foo_lo"), "setter: {names:?}");
    }

    #[test]
    fn cst_bitfield_multiple_fields() {
        let tree =
            cst_tree_full("bitfield Inst : bits(32) = { opcode : 6 .. 0, funct3 : 14 .. 12 }\n");
        let names: Vec<_> =
            tree.top_level_items().iter().map(|id| id.name(&tree).as_str()).collect();
        assert!(names.contains(&"Mk_Inst"));
        assert!(names.contains(&"_get_Inst_opcode"));
        assert!(names.contains(&"_get_Inst_funct3"));
        assert!(names.contains(&"_update_Inst_opcode"));
        assert!(names.contains(&"_set_Inst_funct3"));
    }

    #[test]
    fn cst_bitfield_no_bitfields_unchanged() {
        let tree1 = cst_tree("val x : int\n");
        let tree2 = cst_tree_full("val x : int\n");
        assert_eq!(tree1.len(), tree2.len());
    }

    #[test]
    fn generated_bitfields_keep_each_conditional_declarations_owner() {
        let source = "$ifdef OFF\nbitfield B : bits(8) = { first : 3 .. 0 }\n$else\n/// active docs\nbitfield B : bits(16) = { second : 7 .. 0 }\n$endif\n";
        let raw = cst_tree(source);
        let constructors: Vec<_> = raw
            .top_level_items()
            .iter()
            .filter(|item| item.name(&raw).as_str() == "Mk_B")
            .collect();
        assert_eq!(constructors.len(), 2);
        assert_ne!(constructors[0].name_span(&raw), constructors[1].name_span(&raw));
        for item in constructors {
            let origin = raw.generated[item];
            assert_eq!(item.name_span(&raw), origin.owner.name_span(&raw));
            assert_eq!(item.doc(&raw), origin.owner.doc(&raw));
        }
        let active = cst_tree_full(source);
        assert!(active.find_by_name("_get_B_first").is_none());
        assert!(active.find_by_name("_get_B_second").is_some());
        let constructor = active.find_by_name("Mk_B").unwrap();
        assert!(constructor.signature(&active).contains("bits(16)"));
        assert_eq!(constructor.name_span(&active).start, source.rfind("B :").unwrap());
        assert_eq!(constructor.doc(&active), Some("active docs"));
    }

    #[test]
    fn parses_sail_riscv_prelude_snippet() {
        // Real sail-riscv code with forall constraints, extern bindings,
        // existential type alias, and overload declarations.
        let source = r#"
default Order dec

function not_bit(b : bits(1)) -> bits(1) = if b == 0b1 then 0b0 else 0b1

overload ~ = {not_bool, not_vec, not_bit}

val not : forall ('p : Bool). bool('p) -> bool(not('p))
function not(b) = not_bool(b)

val sub_vec = pure {c: "sub_bits", _: "sub_vec"} : forall 'n. (bits('n), bits('n)) -> bits('n)

overload operator - = {sub_vec}

val quot_positive_round_zero = pure {c: "tdiv_int"} : forall 'n 'm, 'n >= 0 & 'm > 0. (int('n), int('m)) -> int(div('n, 'm))

type nat1 = {'n, 'n > 0. int('n)}

val print_string = pure "print_string" : (string, string) -> unit

register PC : bits(64)

enum Architecture = {RV32, RV64}

scattered function execute
function clause execute(instr) = false
end execute
"#;
        let tree = cst_tree(source);
        let names: Vec<&str> =
            tree.top_level_items().iter().map(|id| id.name(&tree).as_str()).collect();

        // Functions
        assert!(names.contains(&"not_bit"), "should find not_bit: {:?}", names);
        assert!(names.contains(&"not"), "should find not: {:?}", names);

        // Val specs
        assert!(names.contains(&"sub_vec"), "should find sub_vec: {:?}", names);
        assert!(
            names.contains(&"quot_positive_round_zero"),
            "should find quot_positive_round_zero: {:?}",
            names
        );
        assert!(names.contains(&"print_string"), "should find print_string: {:?}", names);

        // Type alias (existential)
        assert!(names.contains(&"nat1"), "should find nat1: {:?}", names);

        // Register
        assert!(names.contains(&"PC"), "should find PC: {:?}", names);

        // Enum
        assert!(names.contains(&"Architecture"), "should find Architecture: {:?}", names);

        // Overloads
        let overloads: Vec<&str> =
            tree.items_of_kind(DefKind::Overload).map(|id| id.name(&tree).as_str()).collect();
        assert!(!overloads.is_empty(), "should find overloads");

        // Scattered
        let scattered: Vec<&str> =
            tree.items_of_kind(DefKind::ScatteredHead).map(|id| id.name(&tree).as_str()).collect();
        assert!(scattered.contains(&"execute"), "should find scattered execute");

        // End marker
        let ends: Vec<&str> =
            tree.items_of_kind(DefKind::EndMarker).map(|id| id.name(&tree).as_str()).collect();
        assert!(ends.contains(&"execute"), "should find end execute");
    }

    #[test]
    fn extracts_fixity_declarations() {
        let source = "infixl 6 op_add\ninfixr 7 op_pow\ninfix 4 op_eq\n";
        let tree = cst_tree(source);
        assert_eq!(tree.fixities.len(), 3, "should extract 3 fixity decls: {:?}", tree.fixities);

        let add = tree.fixities.iter().find(|f| f.operator == "op_add").expect("op_add");
        assert_eq!(add.level, 6);
        assert_eq!(add.assoc, Associativity::Left);

        let pow = tree.fixities.iter().find(|f| f.operator == "op_pow").expect("op_pow");
        assert_eq!(pow.level, 7);
        assert_eq!(pow.assoc, Associativity::Right);

        let eq = tree.fixities.iter().find(|f| f.operator == "op_eq").expect("op_eq");
        assert_eq!(eq.level, 4);
        assert_eq!(eq.assoc, Associativity::None);
    }

    #[test]
    fn fixity_context_maps_correctly() {
        let source = "infixl 6 myop\n";
        let tree = cst_tree(source);
        let ctx = tree.build_fixity_context();
        let (l, r) = ctx.get("myop").expect("myop in context");
        // Level 6 → base_bp = 13, left-assoc → (13, 14)
        assert_eq!(*l, 13);
        assert_eq!(*r, 14);
    }

    #[test]
    fn doc_comment_extracted() {
        let tree = cst_tree("/// This is documentation\nval foo : int\n");
        let id = tree.find_by_name("foo").expect("foo in tree");
        assert_eq!(id.doc(&tree), Some("This is documentation"));
    }

    #[test]
    fn multi_line_doc_comment() {
        let tree = cst_tree("/// Line one\n/// Line two\nval bar : bool\n");
        let id = tree.find_by_name("bar").expect("bar in tree");
        assert_eq!(id.doc(&tree), Some("Line one\nLine two"));
    }

    #[test]
    fn no_doc_comment() {
        let tree = cst_tree("val baz : int\n");
        let id = tree.find_by_name("baz").expect("baz in tree");
        assert!(id.doc(&tree).is_none());
    }

    #[test]
    fn regular_comment_not_doc() {
        let tree = cst_tree("// regular comment\nval qux : int\n");
        let id = tree.find_by_name("qux").expect("qux in tree");
        // Regular comments should NOT be extracted as doc
        assert!(id.doc(&tree).is_none());
    }

    /// Helper: display ItemTree items as structured text for snapshots.
    fn display_item_tree(source: &str) -> String {
        let tree = cst_tree(source);
        tree.top_level_items()
            .iter()
            .map(|id| {
                let doc = id.doc(&tree).unwrap_or("(no doc)");
                format!(
                    "{} {:?}: {} [doc: {}]",
                    id.name(&tree).as_str(),
                    id.def_kind(&tree),
                    id.signature(&tree),
                    doc
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn expect_item_tree_with_docs() {
        use expect_test::expect;
        let display = display_item_tree(
            "/// Add two integers\nval add : (int, int) -> int\nfunction add(x, y) = x + y\n",
        );
        expect![[r#"
            add ValSpec: val add : (int, int) -> int [doc: Add two integers]
            add Function: function add(x, y) [doc: (no doc)]"#]]
        .assert_eq(&display);
    }

    #[test]
    fn expect_item_tree_enum() {
        use expect_test::expect;
        let display = display_item_tree("enum color = { Red, Green, Blue }\n");
        expect![[r#"
            color Enum: enum color = { Red, Green, Blue } [doc: (no doc)]
            color_of_num ValSpec: val color_of_num : forall ('e : Int), 0 <= 'e & 'e <= 2. int('e) -> color [doc: (no doc)]
            num_of_color ValSpec: val num_of_color : color -> range(0, 2) [doc: (no doc)]"#]]
        .assert_eq(&display);
    }

    #[test]
    #[ignore = "slow: parses all 158 sail-riscv files"]
    #[allow(clippy::print_stderr)]
    fn sail_riscv_corpus_parse_check() {
        let model_dir = std::path::PathBuf::from("/home/clair/tinyueng_workplace/sail-riscv/model");
        if !model_dir.exists() {
            eprintln!("sail-riscv not found, skipping");
            return;
        }

        fn collect_sail_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        collect_sail_files(&p, out);
                    } else if p.extension().is_some_and(|e| e == "sail") {
                        out.push(p);
                    }
                }
            }
        }

        let mut sail_files = Vec::new();
        collect_sail_files(&model_dir, &mut sail_files);
        sail_files.sort();

        let mut total_files = 0u32;
        let mut total_items = 0u32;
        let mut total_parse_errors = 0u32;
        let mut error_files: Vec<(String, u32, Vec<String>)> = Vec::new();

        for path in &sail_files {
            let text = match std::fs::read_to_string(path) {
                Ok(t) => t,
                Err(_) => continue,
            };
            total_files += 1;

            let (root, parse_errors) = syntax::parse_text(&text);
            let mut symbols = syntax::preprocess::default_symbols();
            let tree = ItemTree::build_from_cst_full(&root, &mut symbols);
            total_items += tree.len() as u32;

            let n_errs = parse_errors.len() as u32;
            total_parse_errors += n_errs;
            if n_errs > 0 {
                let rel = path.strip_prefix(&model_dir).unwrap_or(path);
                let msgs: Vec<String> = parse_errors
                    .iter()
                    .take(5)
                    .map(|e| {
                        let start = e.offset.saturating_sub(20);
                        let end = std::cmp::min(e.offset + 30, text.len());
                        let snippet = text.get(start..end).unwrap_or("???");
                        format!(
                            "    off {}: {} | ...{}...",
                            e.offset,
                            e.message,
                            snippet.replace('\n', "\\n")
                        )
                    })
                    .collect();
                error_files.push((rel.display().to_string(), n_errs, msgs));
            }
        }

        eprintln!("\n=== sail-riscv Corpus Results ===");
        eprintln!("Files: {}", total_files);
        eprintln!("ItemTree entries: {}", total_items);
        eprintln!("Parse errors: {} across {} files", total_parse_errors, error_files.len());
        eprintln!("Clean: {} / {}", total_files - error_files.len() as u32, total_files);

        error_files.sort_by_key(|b| std::cmp::Reverse(b.1));
        eprintln!("\n=== Files with errors (top 30) ===");
        for (file, count, msgs) in error_files.iter().take(30) {
            eprintln!("{}: {} errors", file, count);
            for msg in msgs {
                eprintln!("{}", msg);
            }
        }
    }
}
