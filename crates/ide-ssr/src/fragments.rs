//! Parse an expression fragment in a minimal Sail binding.
use ide_db::syntax_helpers::node_ext::is_expr_kind;
use syntax::SyntaxNode;

pub(crate) fn expr(s: &str) -> Option<SyntaxNode> {
    // Try wrapping as: `let __ssr_frag__ = <expr>`
    let wrapped = format!("let __ssr_frag__ = {s}");
    let (root, errors) = syntax::parse_text(&wrapped);
    if !errors.is_empty() {
        return None;
    }
    // Find the expression node (child of the let binding, after the '=').
    let node = root.descendants().find(|node| is_expr_kind(node.kind()))?;
    (node.text().to_string().trim() == s.trim()).then_some(node)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FragmentKind {
    Expr,
    Type,
    Item,
    Pattern,
}
impl FragmentKind {
    pub fn split(s: &str) -> (Self, &str) {
        for (prefix, kind) in [
            ("type:", Self::Type),
            ("item:", Self::Item),
            ("pattern:", Self::Pattern),
            ("expr:", Self::Expr),
        ] {
            if let Some(text) = s.trim_start().strip_prefix(prefix) {
                return (kind, text.trim());
            }
        }
        (Self::Expr, s)
    }
    pub fn parse(self, s: &str) -> Option<SyntaxNode> {
        use parser::SyntaxKind as SK;
        if self == Self::Expr {
            return expr(s);
        }
        let wrapped = match self {
            Self::Type => format!("type __ssr_frag__ = {s}"),
            Self::Pattern => format!("let __ssr_frag__ = let {s} = () in ()"),
            Self::Item => s.to_owned(),
            Self::Expr => unreachable!(),
        };
        let (root, errors) = syntax::parse_text(&wrapped);
        if !errors.is_empty() {
            return None;
        }
        let node = match self {
            Self::Type => root.descendants().find(|n| n.kind().is_type_node()),
            Self::Pattern => {
                root.descendants().find(|n| n.kind() == SK::LET_EXPR)?.children().next()
            }
            Self::Item => {
                let mut nodes = root.children();
                let node = nodes.next()?;
                if nodes.next().is_some() {
                    return None;
                }
                Some(node)
            }
            Self::Expr => unreachable!(),
        }?;
        (self.accepts(node.kind()) && node.text().to_string().trim() == s.trim()).then_some(node)
    }
    pub fn accepts(self, kind: parser::SyntaxKind) -> bool {
        use parser::SyntaxKind as SK;
        match self {
            Self::Expr => is_expr_kind(kind),
            Self::Type => kind.is_type_node(),
            Self::Pattern => matches!(
                kind,
                SK::APP_PAT
                    | SK::AS_PAT
                    | SK::BIN_PAT
                    | SK::IDENT_PAT
                    | SK::INDEX_PAT
                    | SK::LIST_PAT
                    | SK::LITERAL_PAT
                    | SK::RANGE_INDEX_PAT
                    | SK::STRUCT_PAT
                    | SK::TUPLE_PAT
                    | SK::TYPED_PAT
                    | SK::TYVAR_PAT
                    | SK::VECTOR_PAT
                    | SK::WILD_PAT
            ),
            Self::Item => matches!(
                kind,
                SK::CALLABLE_DEF
                    | SK::CALLABLE_SPEC
                    | SK::CONSTRAINT_DEF
                    | SK::NAMED_DEF
                    | SK::OUTCOME_DEF
                    | SK::SCATTERED_CLAUSE_DEF
                    | SK::SCATTERED_DEF
                    | SK::TERMINATION_MEASURE_DEF
                    | SK::TYPE_ALIAS_DEF
            ),
        }
    }
}
