//! Position-sensitive lookup for declaration checks. A configured project must
//! never fall back to a whole-workspace name set when a binding is unavailable.
use hir::source_order::{self, bindings, Declaration};
use hir::Name;
use ide_db::FileDb;
use syntax::SyntaxNode;

pub(super) struct OrderedScope<'db> {
    pub db: &'db dyn salsa::Database,
    declarations: Vec<(syntax::TextRange, Declaration)>,
}

/// Signature-comparison checks receive exactly the signature selected before
/// each body, rather than a name map built from the whole file. Keep their
/// existing local shape checks and preserve foreign declaration locations.
pub(super) fn check_selected_signatures(
    defs: &[SyntaxNode],
    scope: Option<&OrderedScope<'_>>,
    check: fn(&[SyntaxNode], &mut Vec<super::Diagnostic>),
    diagnostics: &mut Vec<super::Diagnostic>,
) {
    let Some(scope) = scope else {
        check(defs, diagnostics);
        return;
    };
    for body in defs.iter().filter(|node| node.kind() == syntax::SyntaxKind::CALLABLE_DEF) {
        let Some(name) = syntax::definition_name(body) else { continue };
        let selected =
            scope.resolve(body, &name, bindings::Namespace::Callable).and_then(|declaration| {
                source_order::node(scope.db, declaration)
                    .filter(|node| node.kind() == syntax::SyntaxKind::CALLABLE_SPEC)
                    .map(|node| (declaration, node))
            });
        let start = diagnostics.len();
        let mut inputs = Vec::with_capacity(2);
        if let Some((_, node)) = &selected {
            inputs.push(node.clone());
        }
        inputs.push(body.clone());
        check(&inputs, diagnostics);
        if let Some((declaration, _)) = selected {
            let origin = declaration.file.file_id(scope.db);
            if scope.at(body).is_some_and(|at| at.file.file_id(scope.db) != origin) {
                for diagnostic in &mut diagnostics[start..] {
                    diagnostic.related_in_files.extend(
                        diagnostic
                            .related
                            .drain(..)
                            .map(|(range, message)| (origin, range, message)),
                    );
                }
            }
        }
    }
}

impl<'db> OrderedScope<'db> {
    pub fn new(file: &'db dyn FileDb) -> Option<Self> {
        let (db, file) = file.file_text()?;
        source_order::declarations_for(db, file)?;
        let declarations = syntax::parse_query::semantic_parse_file(db, file)
            .syntax_node()?
            .children()
            .enumerate()
            .map(|(index, node)| (node.text_range(), Declaration { file, index }))
            .collect();
        Some(Self { db, declarations })
    }

    pub fn at(&self, node: &SyntaxNode) -> Option<Declaration> {
        let start = node.text_range().start();
        let index = self.declarations.partition_point(|(range, _)| range.end() <= start);
        self.declarations.get(index).filter(|(range, _)| range.contains(start)).map(|(_, d)| *d)
    }

    pub fn resolve(
        &self,
        node: &SyntaxNode,
        name: &str,
        namespace: bindings::Namespace,
    ) -> Option<Declaration> {
        let at = self.at(node)?;
        bindings::resolve_before(self.db, at, Name::new(name), namespace)
    }

    pub fn node(
        &self,
        at: &SyntaxNode,
        name: &str,
        namespace: bindings::Namespace,
    ) -> Option<SyntaxNode> {
        source_order::node(self.db, self.resolve(at, name, namespace)?)
    }

    pub fn is_constructor(&self, at: &SyntaxNode, name: &str) -> bool {
        self.node(at, name, bindings::Namespace::Value).is_some_and(|node| {
            node.children_with_tokens().any(|token| {
                matches!(
                    token.kind(),
                    syntax::SyntaxKind::KW_ENUM
                        | syntax::SyntaxKind::KW_UNION
                        | syntax::SyntaxKind::KW_NEWTYPE
                )
            })
        })
    }

    /// Resolve only names mentioned by this declaration; never materialize a
    /// complete workspace environment for every declaration check.
    pub fn referenced(
        &self,
        node: &SyntaxNode,
        namespace: bindings::Namespace,
    ) -> std::collections::HashMap<String, SyntaxNode> {
        let names: std::collections::HashSet<_> = node
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| t.kind() == syntax::SyntaxKind::IDENT)
            .map(|t| t.text().to_owned())
            .collect();
        names
            .into_iter()
            .filter_map(|name| self.node(node, &name, namespace).map(|node| (name, node)))
            .collect()
    }

    pub fn call_arities(&self, node: &SyntaxNode, name: &str) -> Option<Vec<(usize, usize)>> {
        let at = self.at(node)?;
        fn signature_arities(
            scope: &OrderedScope<'_>,
            at: Declaration,
            name: &str,
        ) -> Option<Vec<(usize, usize)>> {
            use syntax::SyntaxKind as SK;
            let declaration =
                bindings::resolve(scope.db, at, Name::new(name), bindings::Namespace::Callable)?;
            let node = source_order::node(scope.db, declaration)?;
            let arrow = node.descendants().find(|n| n.kind() == SK::TYPE_ARROW)?;
            let mapping = arrow.children_with_tokens().any(|t| t.kind() == SK::DOUBLE_ARROW);
            let mut out = Vec::new();
            for side in arrow.children().take(if mapping { 2 } else { 1 }) {
                if mapping {
                    let arity = mapping_payload_arity(scope.db, declaration, side, 0)?;
                    if arity == 0 {
                        out.push((0, 1));
                    } else {
                        out.push((arity, arity));
                        if arity > 1 {
                            out.push((1, 1));
                        }
                    }
                    continue;
                }
                out.extend(function_parameter_arities(side));
            }
            (!out.is_empty()).then_some(out)
        }
        // The shared resolver already enforces Sail's bounded overload
        // expansion and rejects cycles/deeper groups. Its results are leaves.
        if let Some(members) = bindings::overload_leaf_members(self.db, at, Name::new(name)) {
            let mut out = Vec::new();
            for member in members {
                out.extend(signature_arities(self, at, member.as_str())?);
            }
            (!out.is_empty()).then_some(out)
        } else {
            signature_arities(self, at, name)
        }
    }
}

/// Known surface arities of an ordinary signature. Mapping payload aliases
/// require declaration-context lookup and are handled separately by the caller.
pub(super) fn function_arities(node: &SyntaxNode) -> Option<Vec<(usize, usize)>> {
    use syntax::SyntaxKind as SK;
    let arrow = node.descendants().find(|n| n.kind() == SK::TYPE_ARROW)?;
    if arrow.children_with_tokens().any(|t| t.kind() == SK::DOUBLE_ARROW) {
        return None;
    }
    Some(function_parameter_arities(arrow.children().next()?))
}

fn function_parameter_arities(side: SyntaxNode) -> Vec<(usize, usize)> {
    use syntax::SyntaxKind as SK;
    let params: Vec<_> =
        if side.kind() == SK::TYPE_TUPLE { side.children().collect() } else { vec![side] };
    let explicit: Vec<_> = params
        .iter()
        .filter(|p| {
            !p.descendants_with_tokens().any(|t| {
                t.as_token().is_some_and(|t| t.kind() == SK::IDENT && t.text() == "implicit")
            })
        })
        .collect();
    let all_unit = explicit.iter().all(|p| p.text().to_string().trim() == "unit");
    vec![(if all_unit { 0 } else { explicit.len() }, params.len())]
}

/// Mapping surface arguments may flatten an alias's tuple payload. Resolve
/// that alias where the signature was declared, including private support.
fn mapping_payload_arity(
    db: &dyn salsa::Database,
    at: Declaration,
    node: SyntaxNode,
    depth: usize,
) -> Option<usize> {
    use syntax::SyntaxKind as SK;
    if depth >= 32 {
        return None;
    }
    match node.kind() {
        SK::TYPE_TUPLE => Some(node.children().count()),
        SK::TYPE_EXISTENTIAL => mapping_payload_arity(
            db,
            at,
            node.children().filter(|n| n.kind().is_type_node()).last()?,
            depth + 1,
        ),
        SK::TYPE_NAMED | SK::TYPE_APP => {
            let head = if node.kind() == SK::TYPE_APP { node.first_child()? } else { node };
            let name = head.text().to_string();
            let name = name.trim();
            if name == "unit" {
                return Some(0);
            }
            if hir::is_builtin_type_name(name) {
                return Some(1);
            }
            let declaration =
                bindings::resolve_before(db, at, Name::new(name), bindings::Namespace::Type)?;
            let definition = source_order::node(db, declaration)?;
            if definition.kind() == SK::TYPE_ALIAS_DEF {
                let rhs = definition.children().find(|n| n.kind().is_type_node())?;
                mapping_payload_arity(db, declaration, rhs, depth + 1)
            } else {
                Some(1)
            }
        }
        _ => None,
    }
}
