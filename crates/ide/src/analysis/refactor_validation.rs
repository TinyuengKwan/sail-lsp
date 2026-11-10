//! Validate a proposed rename in a separate input model. The live database is
//! never mutated. Compare binding identities, including previously unedited
//! uses of the new name that could be captured by the proposed declaration.
use super::shared_edits::PhysicalSymbol;
use super::*;
use ide_db::{
    defs::{Definition, IdentClass},
    source_change::SourceChange,
    text_edit::TextEdit,
};
use salsa::Database as _;

impl Analysis {
    fn preview_change(&self, change: &SourceChange) -> Result<Self, String> {
        let mut inputs = ide_db::apply_change::Change::new();
        for source in self.db.files().all_file_ids() {
            self.db.unwind_if_revision_cancelled();
            let Some(input) = self.db.files().file_text(source) else { continue };
            let mut text = input.text(&self.db).to_string();
            if let Some(edits) = change.source_file_edits.get(&source) {
                let mut ordered: Vec<_> = edits.iter().collect();
                ordered.sort_by_key(|edit| edit.range.start());
                if ordered.windows(2).any(|pair| pair[0].range.end() > pair[1].range.start()) {
                    return Err("Rename contains overlapping edits".into());
                }
                for edit in ordered.iter().rev() {
                    let start = base_db::range_start(edit.range);
                    let end = base_db::range_end(edit.range);
                    if start > end
                        || end > text.len()
                        || !text.is_char_boundary(start)
                        || !text.is_char_boundary(end)
                    {
                        return Err("Rename contains an invalid source range".into());
                    }
                    text.replace_range(start..end, &edit.new_text);
                }
            }
            inputs.change_file(source, text);
        }
        inputs.set_strict_bitvector(base_db::strict_bitvector(&self.db));
        if let Some(project) = base_db::preprocess::WorkspaceSourceProject::try_get(&self.db) {
            inputs.set_source_project(project.project(&self.db).clone());
        } else {
            if let Some(files) = base_db::WorkspaceFiles::try_get(&self.db) {
                inputs.set_workspace_files(
                    files.file_texts(&self.db).iter().map(|file| file.file_id(&self.db)).collect(),
                );
            }
            if let Some(modules) = base_db::module_graph::WorkspaceModules::try_get(&self.db) {
                inputs.set_module_graph(modules.graph(&self.db).clone());
            }
        }
        let mut database = RootDatabase::default();
        database.apply_change(inputs);
        let files = database.files().clone();
        Ok(Self::with_url_map(database, files, self.url_map.clone()))
    }

    fn preview_view(&self, preview: &Self, view: FileId) -> Option<FileId> {
        let source = base_db::preprocess::physical_file(&self.db, view);
        let Some(project) = base_db::preprocess::WorkspaceSourceProject::try_get(&self.db) else {
            return Some(source);
        };
        let plan = hir::preprocess::workspace_plan(&self.db, project);
        let instance = plan.instances.iter().find(|instance| {
            instance.source == source
                && instance.active
                && if instance.primary {
                    view == source
                } else {
                    self.db.include_instance_id(&instance.key) == Some(view)
                }
        })?;
        if instance.primary {
            Some(source)
        } else {
            preview.db.include_instance_id(&instance.key)
        }
    }

    fn rename_binding_at(&self, view: FileId, at: usize) -> Option<Definition> {
        let file = self.file_by_id(view)?;
        let definition = Definition::classify_at(&file, file.position_at(at))?;
        if matches!(definition, Definition::Overload(_)) {
            if let Some(definition) = hir::Semantics::new(&self.db)
                .resolve_overload_callee(file.file_text()?.1, at)
                .and_then(Definition::from_def)
            {
                return Some(definition.canonical(&self.db));
            }
        }
        Some(definition.canonical(&self.db))
    }

    fn expected_rename_binding(
        &self,
        preview: &Self,
        change: &SourceChange,
        view: FileId,
        definition: &Definition,
    ) -> Option<PhysicalSymbol> {
        if let Some(location) = definition.def_location() {
            let (tree, item) = location.item(&self.db)?;
            if let Some(origin) = tree.generated.get(&item) {
                // Generated bindings retain the owner's structural role. A
                // shared overload can become a direct accessor after a rename;
                // compare its selected leaf and regenerate the actual name.
                let old: Vec<_> = tree
                    .top_level_items()
                    .iter()
                    .copied()
                    .filter(|item| {
                        tree.generated.get(item).is_some_and(|g| g.owner == origin.owner)
                    })
                    .collect();
                let ordinal = old.iter().position(|candidate| *candidate == item)?;
                let old_view = location.file_id(&self.db);
                let source = base_db::preprocess::physical_file(&self.db, old_view);
                let new_view = self.preview_view(preview, old_view)?;
                let new_file = preview.file_text(new_view)?;
                let new_tree = hir::def_query::file_item_tree(&preview.db, new_file).as_ref()?;
                let owner_at = relocated(change, source, origin.owner.name_span(tree).start, false);
                let owner = new_tree.top_level_items().iter().copied().find(|item| {
                    !new_tree.generated.contains_key(item)
                        && item.name_span(new_tree).start == owner_at
                        && item.def_kind(new_tree) == origin.owner.def_kind(tree)
                })?;
                let new: Vec<_> = new_tree
                    .top_level_items()
                    .iter()
                    .copied()
                    .filter(|item| new_tree.generated.get(item).is_some_and(|g| g.owner == owner))
                    .collect();
                if old.len() != new.len()
                    || old
                        .iter()
                        .zip(&new)
                        .any(|(a, b)| tree.generated[a].kind != new_tree.generated[b].kind)
                {
                    return None;
                }
                let generated = new[ordinal];
                return Some(PhysicalSymbol::Declaration(
                    std::mem::discriminant(definition),
                    source,
                    generated.name_span(new_tree),
                    generated.name(new_tree).to_string(),
                ));
            }
        }
        Some(relocate_symbol(change, self.physical_definition(view, definition.clone())?))
    }

    pub(super) fn validate_rename_bindings(&self, change: &SourceChange) -> Result<(), String> {
        let names: std::collections::HashSet<String> = change
            .source_file_edits
            .values()
            .flatten()
            .filter_map(|edit| {
                let tokens = parser::tokenize(&edit.new_text);
                (tokens.len() == 1).then(|| ide_db::token_symbol_key(&tokens[0].0)).flatten()
            })
            .collect();
        let mut generated_owners = Vec::new();
        for (&source, edits) in &change.source_file_edits {
            for edit in edits {
                for view in self.instances_at_offset(source, base_db::range_start(edit.range)) {
                    let Some(file) = self.file_by_id(view) else { continue };
                    let Some(definition) = Definition::classify_at(
                        &file,
                        file.position_at(base_db::range_start(edit.range)),
                    ) else {
                        continue;
                    };
                    let Some(location) = definition.def_location() else { continue };
                    let Some((tree, owner)) = location.item(&self.db) else { continue };
                    if tree.generated.values().any(|generated| generated.owner == owner) {
                        let owner = (location.file_id(&self.db), definition);
                        if !generated_owners.contains(&owner) {
                            generated_owners.push(owner);
                        }
                    }
                }
            }
        }
        // A genuinely fresh spelling cannot capture an existing binding. Avoid
        // rebuilding a model for the common fresh-name rename operation.
        let mut existing = Vec::new();
        for id in self.db.files().all_file_ids() {
            let Some(ft) = self.db.files().file_text(id) else { continue };
            let text = ft.text(&self.db);
            if !names.iter().any(|name| text.contains(name)) {
                continue;
            }
            let Some(root) = syntax::parse_query::parse_file(&self.db, ft).syntax_node() else {
                continue;
            };
            for name in &names {
                for (at, _) in text.match_indices(name) {
                    let Some(token) = root.token_at_offset((at as u32).into()).find(|token| {
                        usize::from(token.text_range().start()) == at && token.text() == name
                    }) else {
                        continue;
                    };
                    if !matches!(
                        token.kind(),
                        syntax::SyntaxKind::IDENT | syntax::SyntaxKind::TY_VAR
                    ) {
                        continue;
                    }
                    if change.source_file_edits.get(&id).is_some_and(|edits| {
                        edits.iter().any(|edit| edit.range.contains((at as u32).into()))
                    }) {
                        continue;
                    }
                    existing.push((id, at));
                }
            }
        }
        if existing.is_empty() && generated_owners.is_empty() {
            return Ok(());
        }
        let preview = self.preview_change(change)?;
        for (view, definition) in generated_owners {
            let location = definition.def_location().unwrap();
            let (tree, owner) = location.item(&self.db).unwrap();
            let source = base_db::preprocess::physical_file(&self.db, location.file_id(&self.db));
            let old: Vec<_> = tree
                .top_level_items()
                .iter()
                .copied()
                .filter(|item| {
                    tree.generated.get(item).is_some_and(|generated| generated.owner == owner)
                })
                .collect();
            let new_view =
                self.preview_view(&preview, view).ok_or("Rename removed an include context")?;
            let input = preview.file_text(new_view).ok_or("Rename removed a source")?;
            let new_tree = hir::def_query::file_item_tree(&preview.db, input)
                .as_ref()
                .ok_or("Rename removed a declaration")?;
            let owner_at = relocated(change, source, owner.name_span(tree).start, false);
            let new_owner = new_tree
                .top_level_items()
                .iter()
                .copied()
                .find(|item| {
                    !new_tree.generated.contains_key(item)
                        && item.name_span(new_tree).start == owner_at
                        && item.def_kind(new_tree) == owner.def_kind(tree)
                })
                .ok_or("Rename changed the generating declaration")?;
            let new: Vec<_> = new_tree
                .top_level_items()
                .iter()
                .copied()
                .filter(|item| {
                    new_tree
                        .generated
                        .get(item)
                        .is_some_and(|generated| generated.owner == new_owner)
                })
                .collect();
            if old.len() != new.len()
                || old.iter().zip(&new).any(|(left, right)| {
                    tree.generated[left].kind != new_tree.generated[right].kind
                })
            {
                return Err("Rename changes or suppresses generated bindings; check existing conversion/accessor declarations".into());
            }
            let owner_binding = preview
                .rename_binding_at(new_view, owner_at)
                .and_then(|definition| preview.physical_definition(new_view, definition));
            for (_, new) in
                old.iter().zip(&new).filter(|(old, new)| old.name(tree) != new.name(new_tree))
            {
                let name = new.name(new_tree);
                for (_, file) in preview.all_salsa_files() {
                    let Some((_, candidate)) = file.file_text() else { continue };
                    if !base_db::module_graph::can_reference_file(
                        &preview.db,
                        new_view,
                        candidate.file_id(&preview.db),
                    ) && !base_db::module_graph::can_reference_file(
                        &preview.db,
                        candidate.file_id(&preview.db),
                        new_view,
                    ) {
                        continue;
                    }
                    let Some(other_tree) = file.item_tree() else { continue };
                    for item in other_tree
                        .top_level_items()
                        .iter()
                        .filter(|item| item.name(other_tree) == name)
                    {
                        if candidate.file_id(&preview.db) == new_view && item == new {
                            continue;
                        }
                        if other_tree.generated.get(item).is_some_and(|generated| {
                            candidate.file_id(&preview.db) == new_view
                                && generated.owner == new_owner
                        }) {
                            continue;
                        }
                        if let Some(generated) = other_tree.generated.get(item) {
                            let owner = generated.owner.name_span(other_tree).start;
                            let binding = preview
                                .rename_binding_at(candidate.file_id(&preview.db), owner)
                                .and_then(|definition| {
                                    preview.physical_definition(
                                        candidate.file_id(&preview.db),
                                        definition,
                                    )
                                });
                            // A mapping's val and implementation both carry
                            // generated forward/backward views of one callable.
                            if owner_binding.is_some() && binding == owner_binding {
                                continue;
                            }
                        }
                        // Functions/val specs share the callable namespace.
                        if matches!(
                            item.def_kind(other_tree),
                            hir::DefKind::Function
                                | hir::DefKind::ValSpec
                                | hir::DefKind::Overload
                                | hir::DefKind::Mapping
                        ) {
                            return Err(format!(
                                "Rename conflicts with generated binding `{name}`"
                            ));
                        }
                    }
                }
            }
        }
        let mut candidates = existing;
        for (&source, edits) in &change.source_file_edits {
            candidates.extend(edits.iter().map(|edit| (source, base_db::range_start(edit.range))));
        }
        candidates.sort_unstable();
        candidates.dedup();
        let mut targets = Vec::new();
        for (&source, edits) in &change.source_file_edits {
            for edit in edits {
                for view in self.instances_at_offset(source, base_db::range_start(edit.range)) {
                    let Some(file) = self.file_by_id(view) else { continue };
                    let position = file.position_at(base_db::range_start(edit.range));
                    if let Some(definition) = Definition::classify_at(&file, position) {
                        if matches!(
                            IdentClass::classify_at(&file, position),
                            Some(IdentClass::NameClass(_))
                        ) {
                            targets.push((view, definition, edit.new_text.clone()));
                        }
                    }
                }
            }
        }
        for (source, at) in candidates {
            self.db.unwind_if_revision_cancelled();
            for view in self.instances_at_offset(source, at) {
                let Some(file) = self.file_by_id(view) else { continue };
                let position = file.position_at(at);
                let Some(definition) = Definition::classify_at(&file, position) else { continue };
                if let Some(name) = definition.name(&self.db) {
                    if matches!(
                        IdentClass::classify_at(&file, position),
                        Some(IdentClass::NameClass(_))
                    ) {
                        for (origin, target, new_name) in &targets {
                            if name.as_str() != new_name || !same_namespace(target, &definition) {
                                continue;
                            }
                            if target.canonical(&self.db) == definition.canonical(&self.db) {
                                continue;
                            }
                            if target.is_local()
                                || definition.is_local()
                                || matches!(target, Definition::TypeVar(_))
                                || matches!(definition, Definition::TypeVar(_))
                            {
                                continue;
                            }
                            if base_db::module_graph::can_reference_file(&self.db, *origin, view)
                                || base_db::module_graph::can_reference_file(
                                    &self.db, view, *origin,
                                )
                            {
                                return Err(format!(
                                    "Rename conflicts with an existing declaration `{new_name}`"
                                ));
                            }
                        }
                    }
                }
                let Some(binding) = self.rename_binding_at(view, at) else {
                    continue;
                };
                if self.physical_definition(view, binding.clone()).is_none() {
                    continue;
                }
                let expected = self
                    .expected_rename_binding(&preview, change, view, &binding)
                    .ok_or("Cannot prove the generated binding survives this rename")?;
                let after_view =
                    self.preview_view(&preview, view).ok_or("Rename changed an include context")?;
                let after_at = relocated(change, source, at, false);
                let after = preview
                    .rename_binding_at(after_view, after_at)
                    .and_then(|definition| preview.physical_definition(after_view, definition));
                if after.as_ref() != Some(&expected) {
                    return Err(format!(
                        "Rename would capture or change a binding at {}:{}",
                        self.url_for_file_id(source).map_or("<unknown>", |url| url.as_str()),
                        at
                    ));
                }
            }
        }
        Ok(())
    }
}

fn same_namespace(a: &Definition, b: &Definition) -> bool {
    match (a, b) {
        (Definition::TypeDef(_), Definition::TypeDef(_)) => true,
        (Definition::TypeDef(_), _) | (_, Definition::TypeDef(_)) => false,
        (Definition::Field(a), Definition::Field(b)) => {
            a.parent_name == b.parent_name && a.file_id == b.file_id
        }
        (Definition::Field(_), _) | (_, Definition::Field(_)) => false,
        _ => true,
    }
}

fn relocated(change: &SourceChange, file: FileId, offset: usize, end: bool) -> usize {
    let Some(edits) = change.source_file_edits.get(&file) else { return offset };
    let mut ordered: Vec<&TextEdit> = edits.iter().collect();
    ordered.sort_by_key(|edit| edit.range.start());
    let mut delta = 0isize;
    for edit in ordered {
        let start = base_db::range_start(edit.range);
        let finish = base_db::range_end(edit.range);
        if offset < start || (offset == start && !end) {
            break;
        }
        if offset <= finish {
            return (start as isize + delta) as usize + if end { edit.new_text.len() } else { 0 };
        }
        delta += edit.new_text.len() as isize - (finish - start) as isize;
    }
    (offset as isize + delta) as usize
}

fn relocate_symbol(change: &SourceChange, symbol: PhysicalSymbol) -> PhysicalSymbol {
    let relocate = |file, span: parser::Span, name: String| {
        let renamed = change
            .source_file_edits
            .get(&file)
            .into_iter()
            .flatten()
            .find(|edit| {
                base_db::range_start(edit.range) == span.start
                    && base_db::range_end(edit.range) == span.end
            })
            .map_or(name, |edit| edit.new_text.clone());
        (
            parser::Span::new(
                relocated(change, file, span.start, false),
                relocated(change, file, span.end, true),
            ),
            renamed,
        )
    };
    match symbol {
        PhysicalSymbol::Declaration(kind, file, span, name) => {
            let (span, name) = relocate(file, span, name);
            PhysicalSymbol::Declaration(kind, file, span, name)
        }
        PhysicalSymbol::Local(file, span, name) => {
            let (span, name) = relocate(file, span, name);
            PhysicalSymbol::Local(file, span, name)
        }
        PhysicalSymbol::Builtin(_) => symbol,
    }
}
