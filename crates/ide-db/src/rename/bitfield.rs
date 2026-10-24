//! A field owns three generated functions, but only contributes alternatives
//! to the global field overloads. Rename calls by their selected alternative.
use hir::source_order::{
    self,
    bindings::{self, Namespace},
    Declaration,
};

use super::{RenameError, Result};
use crate::{defs::Definition, text_edit::TextEdit, FileDb};

pub(super) struct BitfieldRename {
    owner: Declaration,
    pub helpers: Vec<(Definition, String, String)>,
    groups: Vec<(String, Vec<usize>, bool)>,
}

impl BitfieldRename {
    pub fn prepare(
        db: &dyn salsa::Database,
        files: &[(base_db::FileId, &dyn FileDb)],
        def: &Definition,
        new_name: &str,
    ) -> Result<Option<Self>> {
        let Definition::Field(field) = def else { return Ok(None) };
        let Some(file) = files.iter().find_map(|(_, file)| {
            let (_, ft) = file.file_text()?;
            (Some(ft.file_id(db)) == field.file_id).then_some(ft)
        }) else {
            return Ok(None);
        };
        let Some(tree) = hir::def_query::file_item_tree(db, file).as_ref() else { return Ok(None) };
        let Some(&owner) = tree.top_level_items().iter().find(|item| {
            item.def_kind(tree) == hir::DefKind::Bitfield && item.name(tree) == &field.parent_name
        }) else {
            return Ok(None);
        };
        if field.name.as_str() == new_name {
            return Ok(None);
        }
        if field.name.as_str() == "bits" || new_name == "bits" {
            return Err(RenameError(
                "The implicit bitfield member `bits` cannot be renamed or replaced.".into(),
            ));
        }
        if parser::single_token(new_name)
            .is_none_or(|(kind, error)| kind != syntax::SyntaxKind::IDENT || error.is_some())
        {
            return Err(RenameError("A bitfield member must be a single identifier.".into()));
        }
        if owner.fields(tree).iter().any(|f| f.name.as_str() == new_name) {
            return Err(RenameError(format!(
                "Bitfield `{}` already has a member `{new_name}`.",
                field.parent_name
            )));
        }
        let at = source_order::at_offset(db, file, owner.name_span(tree).start)
            .ok_or_else(|| RenameError("Bitfield member rename requires a source project or inline module order to validate generated references.".into()))?;
        let (_, get, update, set) =
            hir::bitfield::accessor_names(field.parent_name.as_str(), field.name.as_str());
        let (_, new_get, new_update, new_set) =
            hir::bitfield::accessor_names(field.parent_name.as_str(), new_name);
        let mut helpers = Vec::new();
        for (old, new) in [(get, new_get), (update, new_update), (set, new_set)] {
            let def = hir::def_at_declaration_in_namespace(db, at, &old, Namespace::Callable)
                .and_then(Definition::from_def)
                .ok_or_else(|| {
                    RenameError(format!("Cannot identify generated bitfield function `{old}`."))
                })?;
            helpers.push((def.canonical(db), old, new));
        }
        let declarations = source_order::declarations_for(db, file).unwrap_or_default();
        let mut groups = vec![
            (format!("_mod_{}", field.name), vec![0, 2], false),
            (format!("update_{}", field.name), vec![1], false),
        ];
        // This is a rename-time check over the program, including later
        // contributions: an overload alias observes later extensions too.
        for &declaration in declarations {
            let Some(node) = source_order::node(db, declaration) else { continue };
            let Some(tree) = hir::def_query::file_item_tree(db, declaration.file).as_ref() else {
                continue;
            };
            for item in tree.top_level_items().iter().filter(|item| {
                node.text_range().contains((item.name_span(tree).start as u32).into())
            }) {
                if helpers.iter().any(|(_, _, new)| item.name(tree).as_str() == new) {
                    return Err(RenameError(format!(
                        "Generated bitfield name `{}` already exists.",
                        item.name(tree)
                    )));
                }
                if declaration != at && item.def_kind(tree) == hir::DefKind::Overload {
                    for (group, _, shared) in &mut groups {
                        if item.name(tree).as_str() == group {
                            *shared = true;
                        }
                    }
                }
            }
        }
        Ok(Some(Self { owner: at, helpers, groups }))
    }

    pub fn shared_edits(
        &self,
        db: &dyn salsa::Database,
        file: &dyn FileDb,
    ) -> Result<Vec<TextEdit>> {
        let Some((_, ft)) = file.file_text() else { return Ok(Vec::new()) };
        if source_order::program(db, ft) != source_order::program(db, self.owner.file) {
            return Ok(Vec::new());
        }
        let Some(root) = syntax::parse_query::semantic_parse_file(db, ft).syntax_node() else {
            return Ok(Vec::new());
        };
        let sema = hir::Semantics::new(db);
        let mut edits = Vec::new();
        for token in root.descendants_with_tokens().filter_map(|t| t.into_token()) {
            if token.kind() != syntax::SyntaxKind::IDENT {
                continue;
            }
            let Some((group, indices, shared)) =
                self.groups.iter().find(|(name, _, _)| name == token.text())
            else {
                continue;
            };
            if token.parent_ancestors().any(|n| {
                matches!(
                    n.kind(),
                    syntax::SyntaxKind::ATTRIBUTE | syntax::SyntaxKind::DIRECTIVE_DEF
                )
            }) {
                continue;
            }
            let offset = usize::from(token.text_range().start());
            // Ignore same-spelled values, local bindings, and type names.
            if !matches!(
                Definition::classify_at(file, file.position_at(offset)),
                Some(Definition::Overload(_))
            ) {
                continue;
            }
            let Some(at) = source_order::at_offset(db, ft, offset) else { continue };
            let Some(node) = source_order::node(db, at) else { continue };
            if node.kind() == syntax::SyntaxKind::NAMED_DEF
                && node.children_with_tokens().any(|t| t.kind() == syntax::SyntaxKind::KW_OVERLOAD)
            {
                if syntax::definition_name_span(&node).is_some_and(|span| span.start == offset) {
                    continue;
                }
                if *shared {
                    return Err(RenameError(format!("Cannot safely rename this member: overload alias references shared group `{group}`, whose alternatives can change after the alias declaration. Use explicit accessor names in that alias first.")));
                }
                let replacement = indices
                    .iter()
                    .map(|&i| self.helpers[i].2.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                edits.push(TextEdit { range: token.text_range(), new_text: replacement });
                continue;
            }
            let members =
                bindings::overload_leaf_members(db, at, hir::Name::new(group)).unwrap_or_default();
            let affected = members.iter().any(|member| {
                let Some(selected) = bindings::resolve(db, at, member.clone(), Namespace::Callable)
                else {
                    return false;
                };
                hir::def_at_declaration_in_namespace(
                    db,
                    selected,
                    member.as_str(),
                    Namespace::Callable,
                )
                .and_then(Definition::from_def)
                .is_some_and(|def| {
                    self.helpers.iter().any(|(target, _, _)| &def.canonical(db) == target)
                })
            });
            if !affected {
                continue;
            }
            let chosen = sema.resolve_overload_callee(ft, offset).and_then(Definition::from_def)
                .ok_or_else(|| RenameError(format!("Cannot safely rename this member: the selected implementation of `{group}` is unresolved.")))?
                .canonical(db);
            if let Some((_, _, renamed)) = self.helpers.iter().find(|(def, _, _)| def == &chosen) {
                edits.push(TextEdit { range: token.text_range(), new_text: renamed.clone() });
            }
        }
        Ok(edits)
    }
}
