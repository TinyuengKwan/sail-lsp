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
    let callable = token
        .parent_ancestors()
        .find(|node| matches!(node.kind(), SK::CALLABLE_DEF | SK::SCATTERED_CLAUSE_DEF))?;
    let function = syntax::definition_name(&callable)?;
    let (signature_file, signature) = if hir::source_order::declarations_for(db, file).is_some() {
        let at = hir::source_order::at_offset(db, file, offset)?;
        let signature = hir::source_order::bindings::resolve(
            db,
            at,
            hir::Name::new(&function),
            hir::source_order::bindings::Namespace::Callable,
        )?;
        (signature.file, hir::source_order::node(db, signature)?)
    } else {
        // Without a source order the implementation's `'n` was answered
        // `None` here while the val's own `'n` found its binder above, so
        // the two classified differently and a rename from either side
        // left the other unchanged.
        unordered_signature(db, file, &function)?
    };
    let forall = signature.children().find(|child| child.kind() == SK::TYPE_FORALL)?;
    let list = forall.children().find(|child| child.kind() == SK::TYPE_PARAM_LIST)?;
    Some((signature_file.file_id(db), named_binder(&list, name)?))
}

/// The `val` for `function` in a file set with no source order: the first
/// signature of that name in this file, then in the files it can see.
fn unordered_signature(
    db: &dyn salsa::Database,
    file: FileText,
    function: &str,
) -> Option<(FileText, SyntaxNode)> {
    let ws = base_db::WorkspaceFiles::try_get(db)?;
    let visible =
        hir::visibility::visible_files_for_name(db, ws, file.file_id(db), hir::Name::new(function));
    std::iter::once(file).chain(visible.iter().copied()).find_map(|target| {
        let tree = hir::def_query::file_item_tree(db, target);
        let tree = tree.as_ref()?;
        let item = tree.top_level_items().iter().find(|item| {
            item.def_kind(tree) == hir::DefKind::ValSpec && item.name(tree).as_str() == function
        })?;
        let start = item.span(tree).start;
        let root = syntax::parse_query::semantic_parse_file(db, target).syntax_node()?;
        let node = root
            .token_at_offset((start as u32).into())
            .right_biased()?
            .parent_ancestors()
            .find(|node| node.kind() == SK::CALLABLE_SPEC)?;
        Some((target, node))
    })
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
