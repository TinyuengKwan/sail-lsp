//! Lexical ownership of source-written type/numeric quantifier variables.
use base_db::{FileId, FileText};
use parser::Span;
use syntax::{SyntaxKind as SK, SyntaxNode};

pub(crate) fn binding(
    db: &dyn salsa::Database,
    file: FileText,
    offset: usize,
    name: &str,
) -> Option<(FileId, Span)> {
    let root = syntax::parse_query::semantic_parse_file(db, file).syntax_node()?;
    let token = root
        .token_at_offset((offset as u32).into())
        .find(|token| usize::from(token.text_range().start()) == offset)?;
    for node in token.parent()?.ancestors() {
        let list = if node.kind() == SK::TYPE_PARAM_LIST {
            Some(node.clone())
        } else {
            node.children().find(|child| child.kind() == SK::TYPE_PARAM_LIST)
        };
        if let Some(span) = list.and_then(|list| named_binder(&list, name)) {
            return Some((file.file_id(db), span));
        }
    }
    // A val's quantifiers also bind the implementation's type annotations.
    let at = hir::source_order::at_offset(db, file, offset)?;
    let node = hir::source_order::node(db, at)?;
    if matches!(node.kind(), SK::CALLABLE_DEF | SK::SCATTERED_CLAUSE_DEF) {
        let function = syntax::definition_name(&node)?;
        let signature = hir::source_order::bindings::resolve(
            db,
            at,
            hir::Name::new(&function),
            hir::source_order::bindings::Namespace::Callable,
        )?;
        let node = hir::source_order::node(db, signature)?;
        let forall = node.children().find(|child| child.kind() == SK::TYPE_FORALL)?;
        let list = forall.children().find(|child| child.kind() == SK::TYPE_PARAM_LIST)?;
        return Some((signature.file.file_id(db), named_binder(&list, name)?));
    }
    None
}

fn named_binder(list: &SyntaxNode, name: &str) -> Option<Span> {
    list.descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .find(|token| token.kind() == SK::TY_VAR && token.text() == name)
        .map(|token| {
            Span::new(
                usize::from(token.text_range().start()),
                usize::from(token.text_range().end()),
            )
        })
}
