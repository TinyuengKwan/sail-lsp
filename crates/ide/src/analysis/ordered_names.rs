//! Navigation renders an already resolved declaration; it must not perform a
//! second whole-file/name lookup that changes that declaration's identity.
use super::*;
use crate::navigation_target::NavigationTarget;
use ide_db::defs::{Definition, SymbolKind};

impl Analysis {
    pub(super) fn definition_implementations(&self, def: &Definition) -> Vec<Definition> {
        let canonical = def.canonical(&self.db);
        if !matches!(
            canonical,
            Definition::ValSpec(_)
                | Definition::Function(_)
                | Definition::Mapping(_)
                | Definition::ScatteredHead(_)
                | Definition::Outcome(_)
        ) {
            return Vec::new();
        }
        let Some(name) = canonical.name(&self.db) else { return Vec::new() };
        let Some(loc) = canonical.def_location() else { return Vec::new() };
        let Some((tree, item)) = loc.item(&self.db) else { return Vec::new() };
        let Some(at) =
            hir::source_order::at_offset(&self.db, loc.file_text, item.name_span(tree).start)
        else {
            return Vec::new();
        };
        hir::source_order::bindings::declarations_named_at(
            &self.db,
            at,
            name.clone(),
            hir::source_order::bindings::Namespace::Callable,
        )
        .into_iter()
        .filter_map(|decl| {
            hir::def_at_declaration_in_namespace(
                &self.db,
                decl,
                name.as_str(),
                hir::source_order::bindings::Namespace::Callable,
            )
        })
        .filter_map(Definition::from_def)
        .filter(|d| {
            matches!(
                d,
                Definition::Function(_) | Definition::Mapping(_) | Definition::ScatteredClause(_)
            )
        })
        .filter(|d| d.canonical(&self.db) == canonical)
        .collect()
    }

    pub(super) fn definition_navigation(
        &self,
        from: &dyn FileDb,
        def: Definition,
    ) -> Option<NavigationTarget> {
        let (view, name, kind, full, focus, detail, docs) = if let Some(loc) = def.def_location() {
            let (tree, item) = loc.item(&self.db)?;
            let kind = if item.scattered_type_kind(tree).is_some()
                && item.def_kind(tree) == hir::DefKind::ScatteredHead
            {
                SymbolKind::Enum
            } else {
                crate::goto_definition::item_kind_to_symbol_kind(item.def_kind(tree))
            };
            (
                loc.file_id(&self.db),
                item.name(tree).to_string(),
                kind,
                item.span(tree),
                item.name_span(tree),
                loc.signature_text(&self.db),
                loc.doc_text(&self.db),
            )
        } else {
            match def {
                Definition::Local(local) => (
                    from.file_text()?.1.file_id(&self.db),
                    local.name.to_string(),
                    SymbolKind::Variable,
                    local.span,
                    local.span,
                    None,
                    None,
                ),
                Definition::EnumVariant(member) => {
                    let view = member.file_id?;
                    let ft = self.file_text(view)?;
                    let tree = hir::def_query::file_item_tree(&self.db, ft).as_ref()?;
                    let item = tree.top_level_items().iter().find(|item| {
                        item.name(tree) == &member.parent_name
                            && (item.fields(tree).iter().any(|f| f.name == member.name)
                                || item
                                    .scattered_member(tree)
                                    .is_some_and(|(n, _)| n == member.name.as_str()))
                    })?;
                    let span = item
                        .fields(tree)
                        .iter()
                        .find(|f| f.name == member.name)
                        .map(|f| f.name_span)
                        .or_else(|| item.scattered_member(tree).map(|(_, span)| span))?;
                    (
                        view,
                        member.name.to_string(),
                        SymbolKind::EnumMember,
                        span,
                        span,
                        member.ty_text,
                        None,
                    )
                }
                Definition::Field(field) => {
                    let view = field.file_id?;
                    let tree =
                        hir::def_query::file_item_tree(&self.db, self.file_text(view)?).as_ref()?;
                    let member = tree
                        .top_level_items()
                        .iter()
                        .filter(|i| i.name(tree) == &field.parent_name)
                        .flat_map(|i| i.fields(tree))
                        .find(|f| f.name == field.name)?;
                    (
                        view,
                        field.name.to_string(),
                        SymbolKind::Field,
                        member.name_span,
                        member.name_span,
                        field.ty_text,
                        None,
                    )
                }
                _ => return None,
            }
        };
        let source = base_db::preprocess::physical_file(&self.db, view);
        Some(NavigationTarget {
            file_id: Some(source),
            url: self.url_for_file_id(source)?.clone(),
            name,
            kind,
            full_range: base_db::text_range(full.start, full.end),
            focus_range: base_db::text_range(focus.start, focus.end),
            detail,
            docs,
            children: Vec::new(),
        })
    }
}
