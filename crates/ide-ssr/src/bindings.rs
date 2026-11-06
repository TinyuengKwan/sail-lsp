//! Fragment binding identities come from the same lowering and expression
//! scopes as inference. Offsets refer to physical fragment tokens.
use rustc_hash::FxHashMap;
use syntax::SyntaxNode;
#[derive(Debug, Clone, Default)]
pub(crate) struct Bindings {
    pub declarations: FxHashMap<usize, String>,
    pub references: FxHashMap<usize, Option<usize>>,
}
impl Bindings {
    pub fn of_with_globals(node: &SyntaxNode, is_global: impl Fn(&hir::Name) -> bool) -> Self {
        use parser::SyntaxKind as SK;
        let (body, map) = if node.kind() == SK::CALLABLE_DEF {
            let Some(body) = hir::CallableBodies::cst_callable_body_node(node) else {
                return Self::default();
            };
            let params = node.children().find(|n| n.kind() == SK::PARAM_LIST);
            hir::Body::lower_body_with_params(&body, params.as_ref())
        } else if crate::fragments::FragmentKind::Pattern.accepts(node.kind()) {
            hir::Body::lower_body_with_params(node, Some(node))
        } else {
            hir::Body::lower_body(node)
        };
        let mut out = Self::default();
        let mut bodies = vec![(std::sync::Arc::new(body), std::sync::Arc::new(map))];
        if matches!(node.kind(), SK::NAMED_DEF | SK::SCATTERED_CLAUSE_DEF) {
            if let Some(parent) = node.parent() {
                let callables = hir::CallableBodies::from_cst(&parent);
                bodies.extend(
                    callables
                        .entries()
                        .iter()
                        .filter(|body| {
                            body.def_span.start >= usize::from(node.text_range().start())
                                && body.def_span.end <= usize::from(node.text_range().end())
                        })
                        .map(|body| (body.body.clone(), body.source_map.clone())),
                );
            }
        }
        for (body, map) in bodies {
            let scopes = hir::ExprScopes::new_with_globals(&body, &is_global);
            for (id, pat) in body.iter_pats() {
                let name = match pat {
                    hir::Pat::Bind(name) | hir::Pat::TypeVar(name) => Some(name),
                    hir::Pat::As { binding, .. } => Some(binding),
                    _ => None,
                };
                if let (Some(name), Some(span)) = (name, map.pat_syntax(id)) {
                    if scopes.binds_pattern(id) {
                        let offset = node
                            .descendants_with_tokens()
                            .filter_map(|e| e.into_token())
                            .filter(|t| {
                                t.text() == name
                                    && usize::from(t.text_range().start()) >= span.start
                                    && usize::from(t.text_range().end()) <= span.end
                            })
                            .last()
                            .map_or(span.start, |t| usize::from(t.text_range().start()));
                        out.declarations.insert(offset, name.clone());
                    }
                }
            }
            for (id, expr) in body.iter_exprs() {
                if let hir::Expr::Ident(name) = expr {
                    if let Some(span) = map.expr_syntax(id) {
                        let binding = scopes
                            .scope_for(id)
                            .and_then(|scope| {
                                scopes.resolve_name_in_scope(scope, &hir::Name::new(name))
                            })
                            .and_then(|entry| map.pat_syntax(entry.pat()))
                            .map(|span| {
                                out.declarations
                                    .keys()
                                    .filter(|&&offset| span.start <= offset && offset < span.end)
                                    .max()
                                    .copied()
                                    .unwrap_or(span.start)
                            });
                        out.references.insert(span.start, binding);
                    }
                }
            }
        }
        let mut type_bindings = Vec::new();
        for param in node.descendants().filter(|n| n.kind() == SK::TYPE_PARAM) {
            let scope = param
                .ancestors()
                .find(|n| n.kind() == SK::TYPE_PARAM_LIST)
                .and_then(|n| n.parent());
            let Some(scope) = scope else { continue };
            for token in param
                .descendants_with_tokens()
                .filter_map(|e| e.into_token())
                .take_while(|t| t.kind() != SK::COLON)
                .filter(|t| matches!(t.kind(), SK::TY_VAR | SK::IDENT) && t.text() != "constant")
            {
                let start = usize::from(token.text_range().start());
                out.declarations.insert(start, token.text().to_owned());
                type_bindings.push((scope.text_range(), start, token.text().to_owned()));
            }
        }
        let mut type_references = references(node);
        type_references.extend(
            node.descendants_with_tokens()
                .filter_map(|e| e.into_token())
                .filter(|t| t.kind() == SK::TY_VAR)
                .map(|t| (t, true)),
        );
        for (token, _) in type_references {
            let start = usize::from(token.text_range().start());
            if out.declarations.contains_key(&start) {
                continue;
            }
            if token.kind() == SK::TY_VAR || token.parent().is_some_and(|p| p.kind().is_type_node())
            {
                let binding = type_bindings
                    .iter()
                    .filter(|(scope, _, name)| {
                        name == token.text() && scope.contains_range(token.text_range())
                    })
                    .min_by_key(|(scope, _, _)| scope.len())
                    .map(|(_, binding, _)| *binding);
                out.references.insert(start, binding);
            }
        }
        out
    }
}

/// Fixed references in a fragment. Declaration names, field labels and bound
/// patterns are handled separately; type names always use the type namespace.
pub(crate) fn references(node: &SyntaxNode) -> Vec<(syntax::SyntaxToken, bool)> {
    use parser::SyntaxKind as SK;
    node.descendants()
        .filter_map(|n| {
            let ty = matches!(n.kind(), SK::TYPE_NAMED | SK::TYPE_APP | SK::TYPE_VAR);
            if !ty && !matches!(n.kind(), SK::IDENT_EXPR | SK::APP_PAT) {
                return None;
            }
            let token = n
                .children_with_tokens()
                .filter_map(|e| e.into_token())
                .find(|t| matches!(t.kind(), SK::IDENT | SK::TY_VAR))?;
            Some((token, ty))
        })
        .collect()
}
