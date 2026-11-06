//! Physical source edits must agree in every active preprocessing context.
//! Identity includes the resolved declaration location and namespace, never
//! just its spelling. A selected read context cannot waive this check.
use super::*;
use ide_db::{defs::Definition, source_change::SourceChange};
use salsa::Database as _;

#[derive(Clone, Debug, PartialEq, Eq)]
enum PhysicalSymbol {
    Declaration(std::mem::Discriminant<Definition>, FileId, parser::Span, String),
    Local(FileId, parser::Span, String),
    Builtin(String),
}

impl Analysis {
    fn same_import(
        a: &Option<ide_db::imports::import_assets::ImportRequest>,
        b: &Option<ide_db::imports::import_assets::ImportRequest>,
    ) -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                a.source == b.source
                    && a.target == b.target
                    && a.offset == b.offset
                    && a.name == b.name
                    && a.source_hash == b.source_hash
                    && a.target_hash == b.target_hash
            }
            _ => false,
        }
    }

    fn physical_change(&self, mut change: SourceChange) -> SourceChange {
        let mut physical = SourceChange::new();
        physical.is_snippet = change.is_snippet;
        for (view, edits) in change.source_file_edits {
            let source = base_db::preprocess::physical_file(&self.db, view);
            match physical.source_file_edits.entry(source) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(edits);
                }
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    for edit in edits {
                        if !entry.get().contains(&edit) {
                            entry.get_mut().push(edit);
                        }
                    }
                }
            }
        }
        for creation in &mut change.file_creations {
            creation.anchor = base_db::preprocess::physical_file(&self.db, creation.anchor);
        }
        physical.file_creations = change.file_creations;
        Self::normalize_edits(&mut physical);
        physical
    }

    fn same_change(a: &SourceChange, b: &SourceChange) -> bool {
        a.is_snippet == b.is_snippet
            && a.source_file_edits == b.source_file_edits
            && a.file_creations.len() == b.file_creations.len()
            && a.file_creations
                .iter()
                .zip(&b.file_creations)
                .all(|(a, b)| a.anchor == b.anchor && a.path == b.path && a.contents == b.contents)
    }

    fn shared_change_symbols_agree(&self, change: &SourceChange) -> bool {
        change.source_file_edits.iter().all(|(&source, edits)| {
            edits.iter().all(|edit| self.ensure_shared_range_symbols(source, edit.range).is_ok())
        })
    }

    pub(super) fn assists_inner(
        &self,
        url: &url::Url,
        range: base_db::TextRange,
        config: &ide_assists::AssistConfig,
    ) -> Vec<ide_db::assists::Assist> {
        let Some(source) = self.url_map.lookup_file_id(url) else { return Vec::new() };
        let views = self.instances_at_offset(source, base_db::range_start(range));
        let mut all: Vec<Vec<_>> = views
            .iter()
            .map(|&view| {
                let mut assists = self.in_view(source, view).assists_in_view(url, range, config);
                for assist in &mut assists {
                    assist.source_change =
                        assist.source_change.take().map(|change| self.physical_change(change));
                }
                assists
            })
            .collect();
        if all.is_empty() {
            return Vec::new();
        }
        let mut selected = all.remove(0);
        selected.retain(|assist| {
            let Some(change) = &assist.source_change else { return all.is_empty() };
            // auto_include has already previewed every include visit through
            // import_assets::resolve, and resolve repeats that check later.
            // The missing name cannot have a pre-edit declaration identity.
            (assist.import.is_some()
                || (self.ensure_shared_range_symbols(source, assist.target).is_ok()
                    && self.shared_change_symbols_agree(change)))
                && all.iter().all(|others| {
                    others.iter().any(|other| {
                        assist.id == other.id
                            && assist.target == other.target
                            && assist.label == other.label
                            && Self::same_import(&assist.import, &other.import)
                            && other
                                .source_change
                                .as_ref()
                                .is_some_and(|other| Self::same_change(change, other))
                    })
                })
        });
        selected
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn code_actions_inner(
        &self,
        url: &url::Url,
        range: base_db::TextRange,
        diagnostics: &[ide_diagnostics::Diagnostic],
        format: &ide_db::ide_types::FormatOptions,
        config: &ide_assists::AssistConfig,
    ) -> Vec<CodeAction> {
        let Some(source) = self.url_map.lookup_file_id(url) else { return Vec::new() };
        let views = self.instances_at_offset(source, base_db::range_start(range));
        let mut all: Vec<Vec<_>> = views
            .iter()
            .map(|&view| {
                let mut actions = self.in_view(source, view).code_actions_in_view(
                    url,
                    range,
                    diagnostics,
                    format,
                    config,
                );
                for action in &mut actions {
                    action.source_change =
                        self.physical_change(std::mem::take(&mut action.source_change));
                }
                actions
            })
            .collect();
        if all.is_empty() {
            return Vec::new();
        }
        let mut selected = all.remove(0);
        selected.retain(|action| {
            (action.import.is_some()
                || (action
                    .semantic_range
                    .is_none_or(|range| self.ensure_shared_range_symbols(source, range).is_ok())
                    && (action.semantic_range.is_none()
                        || self.shared_change_symbols_agree(&action.source_change))))
                && all.iter().all(|others| {
                    others.iter().any(|other| {
                        action.title == other.title
                            && action.kind == other.kind
                            && action.for_diagnostic == other.for_diagnostic
                            && action.is_fix_all == other.is_fix_all
                            && Self::same_import(&action.import, &other.import)
                            && action.semantic_range == other.semantic_range
                            && Self::same_change(&action.source_change, &other.source_change)
                    })
                })
        });
        selected
    }

    /// Compare every resolved name touched by a shared edit, not merely the
    /// symbol where the operation started. Missing resolution is not equality.
    fn ensure_shared_range_symbols(
        &self,
        source: FileId,
        range: base_db::TextRange,
    ) -> Result<(), String> {
        let views = self.instances_at_offset(source, base_db::range_start(range));
        if views.len() <= 1 {
            return Ok(());
        }
        let ft = self.file_text(source).ok_or("Shared edit target disappeared")?;
        let root = syntax::parse_query::parse_file(&self.db, ft)
            .syntax_node()
            .ok_or("Shared edit target has no syntax tree")?;
        let tokens: Vec<_> = root
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| range.contains_range(t.text_range()))
            .filter(|t| {
                use syntax::SyntaxKind as SK;
                matches!(t.kind(), SK::IDENT | SK::TY_VAR | SK::OP_IDENT)
                    || t.parent().is_some_and(|p| p.kind() == SK::BIN_EXPR)
                        && !t.kind().is_trivia()
                        && !matches!(t.kind(), SK::L_PAREN | SK::R_PAREN)
            })
            .collect();
        for token in tokens {
            let offset = usize::from(token.text_range().start());
            let active = self.instances_at_offset(source, offset);
            if active != views {
                return Err(
                    "Edit crosses conditional regions with different include contexts".into()
                );
            }
            let mut expected = None;
            for &view in &views {
                let symbol = self
                    .physical_symbol(view, offset)
                    .or_else(|| {
                        let ft = self.file_text(view)?;
                        let sema = hir::Semantics::new(&self.db);
                        let def = sema.resolve_path(&self.db, ft, offset, token.text()).or_else(
                            || {
                                sema.resolve_callable_at(ft, offset, token.text())
                                    .map(hir::PathResolution::Def)
                            },
                        )?;
                        self.physical_definition(
                            view,
                            Definition::from_path_resolution(def)?.canonical(&self.db),
                        )
                    })
                    .ok_or("Cannot prove binding identity in multiple include contexts")?;
                if expected.as_ref().is_some_and(|expected| expected != &symbol) {
                    return Err("An edited token resolves to different bindings in multiple include contexts".into());
                }
                expected = Some(symbol);
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn structural_replace_in_all_contexts(
        &self,
        source: FileId,
        position: ide_db::LineCol,
        query: &str,
        parse_only: bool,
        selections: &[base_db::TextRange],
        excluded: &std::collections::HashSet<FileId>,
    ) -> Result<SourceChange, String> {
        let mut origins = self.position_instances(source, position);
        if origins.is_empty() {
            origins.push(source);
        }
        let mut finders = Vec::new();
        let mut expected_bindings = None;
        for &origin in &origins {
            self.db.unwind_if_revision_cancelled();
            let file = self.file_by_id(origin).ok_or("SSR invocation file disappeared")?;
            let ft = self.file_text(origin).ok_or("SSR invocation text disappeared")?;
            let finder = ide_ssr::MatchFinder::new(&self.db, ft, file.offset_at(&position), query)
                .map_err(|e| e.to_string())?;
            if origins.len() > 1 {
                let bindings = finder
                    .resolved_bindings()
                    .into_iter()
                    .map(|(name, ty, resolution)| {
                        let def = Definition::from_path_resolution(resolution.clone())?
                            .canonical(&self.db);
                        Some((name.to_owned(), ty, self.physical_definition(origin, def)?))
                    })
                    .collect::<Option<Vec<_>>>()
                    .ok_or("Cannot prove SSR rule bindings in multiple include contexts")?;
                if expected_bindings.as_ref().is_some_and(|expected| expected != &bindings) {
                    return Err(
                        "SSR rule resolves different bindings in multiple include contexts".into(),
                    );
                }
                expected_bindings = Some(bindings);
            }
            finders.push(finder);
        }
        let mut change = SourceChange::new();
        if parse_only {
            return Ok(change);
        }
        let mut proposals = std::collections::HashMap::new();
        for (_, file) in self.all_salsa_files() {
            self.db.unwind_if_revision_cancelled();
            let Some((_, ft)) = file.file_text() else { continue };
            let view = ft.file_id(&self.db);
            let physical = base_db::preprocess::physical_file(&self.db, view);
            if excluded.contains(&physical) || (!selections.is_empty() && physical != source) {
                continue;
            }
            let Some(root) = syntax::parse_query::semantic_parse_file(&self.db, ft).syntax_node()
            else {
                continue;
            };
            let mut edits = Vec::new();
            for finder in &finders {
                for edit in finder.edits_for_tree_in_ranges(&root, ft, selections) {
                    if !edits.contains(&edit) {
                        if edits.iter().any(|other: &ide_db::text_edit::TextEdit| {
                            other.range.intersect(edit.range).is_some_and(|r| !r.is_empty())
                                || other.range == edit.range
                        }) {
                            return Err(
                                "SSR produces conflicting edits across include contexts".into()
                            );
                        }
                        edits.push(edit);
                    }
                }
            }
            proposals.insert(view, edits);
        }
        let mut checked = std::collections::HashSet::new();
        for (&view, edits) in &proposals {
            let physical = base_db::preprocess::physical_file(&self.db, view);
            for edit in edits {
                for context in self.instances_at_offset(physical, base_db::range_start(edit.range))
                {
                    if !proposals.get(&context).is_some_and(|edits| edits.contains(edit)) {
                        return Err(
                            "SSR produces different edits in multiple include contexts".into()
                        );
                    }
                }
                if checked.insert((physical, edit.range)) {
                    self.ensure_shared_range_symbols(physical, edit.range)?;
                    change.source_file_edits.entry(physical).or_default().push(edit.clone());
                }
            }
        }
        Self::normalize_edits(&mut change);
        for (&file, edits) in &change.source_file_edits {
            let mut text =
                self.file_text(file).ok_or("SSR target disappeared")?.text(&self.db).to_string();
            for edit in edits.iter().rev() {
                text.replace_range(
                    base_db::range_start(edit.range)..base_db::range_end(edit.range),
                    &edit.new_text,
                );
            }
            if !syntax::parse_text(&text).1.is_empty() {
                return Err("SSR replacement would leave an invalid Sail file".into());
            }
        }
        Ok(change)
    }

    /// One physical declaration can have several semantic IDs. Search each
    /// selected identity, rather than losing references in the other roots.
    /// The caller still validates every affected token before returning edits.
    pub(super) fn rename_candidates(&self, view: FileId, def: Definition) -> Vec<Definition> {
        let canonical = def.canonical(&self.db);
        let Some(PhysicalSymbol::Declaration(_, source, span, _)) =
            self.physical_definition(view, canonical.clone())
        else {
            return vec![def];
        };
        let expected = self.physical_definition(view, canonical);
        let mut candidates = vec![def];
        for instance in self.instances_at_offset(source, span.start) {
            let Some(file) = self.file_by_id(instance) else { continue };
            let Some(other) = Definition::classify_at(&file, file.position_at(span.start)) else {
                continue;
            };
            if self.physical_definition(instance, other.canonical(&self.db)) == expected
                && !candidates.contains(&other)
            {
                candidates.push(other);
            }
        }
        candidates
    }

    fn physical_symbol(&self, view: FileId, offset: usize) -> Option<PhysicalSymbol> {
        let file = self.file_by_id(view)?;
        let def = Definition::classify_at(&file, file.position_at(offset))?.canonical(&self.db);
        self.physical_definition(view, def)
    }

    fn physical_definition(&self, view: FileId, def: Definition) -> Option<PhysicalSymbol> {
        let name = def.name(&self.db)?.to_string();
        let kind = std::mem::discriminant(&def);
        let physical = |view| base_db::preprocess::physical_file(&self.db, view);
        let (view, span) = if let Some(loc) = def.def_location() {
            let (tree, item) = loc.item(&self.db)?;
            (loc.file_id(&self.db), item.name_span(tree))
        } else {
            match def {
                Definition::Local(local) => {
                    return Some(PhysicalSymbol::Local(physical(view), local.span, name))
                }
                Definition::Field(field) => (field.file_id?, field.declaration_span),
                Definition::EnumVariant(member) => (member.file_id?, member.declaration_span),
                Definition::BuiltinType(_) => return Some(PhysicalSymbol::Builtin(name)),
                Definition::TypeVar(_) if ide_db::defs::BuiltinType::from_name(&name).is_some() => {
                    return Some(PhysicalSymbol::Builtin(name))
                }
                // Unresolved and ownerless type variables are not proof that
                // two contexts refer to the same declaration.
                _ => return None,
            }
        };
        Some(PhysicalSymbol::Declaration(kind, physical(view), span, name))
    }

    fn in_view(&self, source: FileId, view: FileId) -> Self {
        let mut analysis = self.clone();
        analysis.context = Some((source, view));
        analysis
    }

    fn normalize_edits(change: &mut SourceChange) {
        for edits in change.source_file_edits.values_mut() {
            // LSP permits multiple insertions at one point. Their array order
            // and multiplicity are meaningful; only duplicate semantic views
            // may be deduplicated when mapping onto a physical file.
            edits.sort_by_key(|edit| (edit.range.start(), edit.range.end()));
        }
    }

    pub(super) fn rename_in_all_contexts(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
        new_name: &str,
    ) -> Result<Option<SourceChange>, String> {
        let Some(source) = self.url_map.lookup_file_id(url) else { return Ok(None) };
        let views = self.position_instances(source, position);
        let first = views.first().copied().unwrap_or(source);
        let mut expected = self.in_view(source, first).rename_inner(url, position, new_name)?;
        if let Some(change) = &mut expected {
            Self::normalize_edits(change);
        }
        let Some(change) = &expected else { return Ok(None) };
        if views.len() > 1 {
            let file = self.file_by_id(source).ok_or("Rename source disappeared")?;
            let offset = file.offset_at(&position);
            let original = self.physical_symbol(first, offset).ok_or(
                "Cannot prove the same binding in multiple include contexts; rename is ambiguous.",
            )?;
            for &view in &views[1..] {
                if self.physical_symbol(view, offset).as_ref() != Some(&original) {
                    return Err("This token resolves to different bindings in multiple include contexts; rename is ambiguous.".into());
                }
                let mut actual = self
                    .in_view(source, view)
                    .rename_inner(url, position, new_name)?
                    .ok_or("Rename is unavailable in one of multiple include contexts")?;
                Self::normalize_edits(&mut actual);
                if actual.source_file_edits != change.source_file_edits {
                    return Err(
                        "Rename produces different edits in multiple include contexts.".into()
                    );
                }
            }
        }
        // Inspect each edited token in all its active contexts, even when
        // invoked from an unshared parent. Do not recursively rename at a
        // generated helper: derived-name edits belong to their owner operation.
        for (&physical, edits) in &change.source_file_edits {
            for edit in edits {
                let offset = base_db::range_start(edit.range);
                let contexts = self.instances_at_offset(physical, offset);
                if contexts.len() <= 1 {
                    continue;
                }
                let mut identity = None;
                for view in contexts {
                    let symbol = self.physical_symbol(view, offset).ok_or(
                        "Cannot prove the same binding in multiple include contexts; rename is ambiguous.")?;
                    if identity.as_ref().is_some_and(|expected| expected != &symbol) {
                        return Err("This token resolves to different bindings in multiple include contexts; rename is ambiguous.".into());
                    }
                    identity = Some(symbol);
                }
            }
        }
        Ok(expected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::hierarchy::tests::{setup, snapshot};
    use ide_db::text_edit::TextEdit;

    #[test]
    fn physical_edits_preserve_coincident_insertion_order_and_multiplicity() {
        let original = "function f() -> int = 1\n";
        let (host, map, _) = setup(&[("main.sail", original)], &[0], vec![]);
        let analysis = snapshot(&host, &map);
        let at = original.find("1\n").unwrap();
        let mut change = SourceChange::new();
        change.source_file_edits.insert(
            FileId::from_raw(0),
            vec![
                TextEdit { range: base_db::text_range(at, at), new_text: "(/*z*/".into() },
                TextEdit { range: base_db::text_range(at, at), new_text: "(".into() },
                TextEdit { range: base_db::text_range(at, at), new_text: "(".into() },
                TextEdit { range: base_db::text_range(at + 1, at + 1), new_text: ")))".into() },
            ],
        );
        let change = analysis.physical_change(change);
        let mut updated = original.to_string();
        for edit in change.source_file_edits[&FileId::from_raw(0)].iter().rev() {
            updated.replace_range(
                base_db::range_start(edit.range)..base_db::range_end(edit.range),
                &edit.new_text,
            );
        }
        assert_eq!(updated, "function f() -> int = (/*z*/((1)))\n");
        assert!(syntax::parse_text(&updated).1.is_empty());
    }
}
