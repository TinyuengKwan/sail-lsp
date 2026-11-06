//! Explain the same semantic views and name resolution used by IDE requests.
use super::*;
use ide_db::defs::Definition;
use serde_json::{json, Value};

impl Analysis {
    /// Stable preprocessing identities and their existing semantic views.
    pub fn include_contexts(
        &self,
        uri: &url::Url,
    ) -> crate::Cancellable<Vec<(FileId, base_db::preprocess::InstanceKey, url::Url)>> {
        self.with_db(|a| {
            let Some(source) = a.file_id(uri) else { return Vec::new() };
            let Some(project) = base_db::preprocess::WorkspaceSourceProject::try_get(&a.db) else {
                return Vec::new();
            };
            hir::preprocess::workspace_plan(&a.db, project)
                .instances
                .iter()
                .filter_map(|instance| {
                    if instance.source != source || !instance.active {
                        return None;
                    }
                    let view = if instance.primary {
                        source
                    } else {
                        a.db.include_instance_id(&instance.key)?
                    };
                    Some((
                        view,
                        instance.key.clone(),
                        a.url_for_file_id(instance.key.root)?.clone(),
                    ))
                })
                .collect()
        })
    }

    pub fn with_preferred_views(
        mut self,
        views: std::collections::HashMap<FileId, FileId>,
    ) -> Self {
        self.preferred_views = std::sync::Arc::new(views);
        self
    }

    pub fn explain_position(
        &self,
        file: FileId,
        position: ide_db::LineCol,
    ) -> crate::Cancellable<Value> {
        self.with_db(|a| a.explain_position_inner(file, position))
    }

    fn explain_position_inner(&self, file: FileId, position: ide_db::LineCol) -> Value {
        let Some(source) = self.file_by_id(file) else { return Value::Null };
        let offset = source.offset_at(&position);
        let membership = base_db::preprocess::project_membership(&self.db, file);
        let Some(ft) = self.file_text(file) else { return Value::Null };
        let root = syntax::parse_query::parse_file(&self.db, ft).syntax_node();
        let region = root.as_ref().map_or(0, |root| {
            root.children()
                .filter(|node| {
                    usize::from(node.text_range().end()) <= offset
                        && syntax::preprocess::Event::from_node(node).is_some()
                })
                .count()
        });
        let mut contexts = Vec::new();
        for view in base_db::preprocess::file_instances(&self.db, file) {
            let active = base_db::preprocess::file_regions(&self.db, view)
                .is_none_or(|regions| regions.get(region).copied().unwrap_or(false));
            let Some(sf) = self.file_by_id(view) else { continue };
            let definition = active.then(|| Definition::classify_at(&sf, position)).flatten();
            let name = definition
                .as_ref()
                .and_then(|def| def.name(&self.db))
                .map(|name| name.to_string())
                .or_else(|| {
                    sf.token_at(position).and_then(|(token, _)| ide_db::token_symbol_key(token))
                });
            let resolved =
                definition.as_ref().is_some_and(|def| !matches!(def, Definition::Unresolved(_)));
            let selected = definition.as_ref().and_then(|def| def.def_location()).and_then(|loc| {
                let (tree, item) = loc.item(&self.db)?;
                Some((loc.file_id(&self.db), item.name_span(tree).start))
            });
            let module =
                base_db::module_graph::WorkspaceModules::try_get(&self.db).and_then(|graph| {
                    graph.graph(&self.db).modules.iter().position(|m| m.files.contains(&view))
                });
            let scope = base_db::module_graph::file_scope(&self.db, view);
            let mut candidates = Vec::new();
            if let Some(name) = &name {
                for (uri, candidate) in self.all_salsa_files() {
                    let Some(tree) = candidate.item_tree() else { continue };
                    let Some((_, candidate_ft)) = candidate.file_text() else { continue };
                    let candidate_id = candidate_ft.file_id(&self.db);
                    for &item in tree
                        .top_level_items()
                        .iter()
                        .filter(|item| item.name(tree).as_str() == name)
                    {
                        let own = base_db::module_graph::same_module(&self.db, view, candidate_id);
                        let reason = if selected == Some((candidate_id, item.name_span(tree).start))
                        {
                            "selected declaration"
                        } else if item.visibility(tree) == hir::visibility::RawVisibility::Private
                            && !own
                        {
                            "private declaration in another module"
                        } else if !base_db::module_graph::can_reference_file(
                            &self.db,
                            view,
                            candidate_id,
                        ) && item.def_kind(tree) != hir::DefKind::Overload
                        {
                            "outside this module and its direct requirements"
                        } else {
                            "not selected at this position (declaration order, namespace, overload, or shadowing)"
                        };
                        candidates.push(json!({"uri":uri.as_str(), "offset":item.name_span(tree).start, "reason":reason}));
                    }
                }
            }
            let visible_files: Vec<_> = scope
                .into_iter()
                .flat_map(|s| s.visible.iter().copied().collect::<Vec<_>>())
                .filter_map(|id| {
                    self.url_for_file_id(base_db::preprocess::physical_file(&self.db, id))
                        .map(|url| url.as_str().to_owned())
                })
                .collect();
            contexts.push(json!({"view":view.index(), "selected":self.preferred_views.get(&file) == Some(&view), "moduleIndex":module, "active":active,
                "name":name, "resolved":resolved,
                "resolution": if !active { "inactive conditional source region" }
                    else if membership == Some(false) { "file is outside the loaded project" }
                    else if definition.as_ref().is_some_and(Definition::is_local) { "local binding or type variable" }
                    else if resolved { "resolved in the active semantic scope" }
                    else { "no selected declaration at this position" },
                "visibleFiles":visible_files,"candidates":candidates}));
        }
        json!({"file":self.url_for_file_id(file).map(|url| url.as_str()), "projectMember":membership, "contexts":contexts})
    }
}

#[cfg(test)]
mod tests {
    use super::super::hierarchy::tests::{setup, snapshot};
    use super::*;
    use base_db::module_graph::ModuleData;

    #[test]
    fn explains_private_and_non_required_declarations_using_semantic_scope() {
        for (declaration, required, reason, resolved) in [
            (
                "private val helper : int -> int\n",
                true,
                "private declaration in another module",
                false,
            ),
            (
                "val helper : int -> int\n",
                false,
                "outside this module and its direct requirements",
                false,
            ),
            ("val helper : int -> int\n", true, "selected declaration", true),
        ] {
            let main = "function use(x : int) -> int = helper(x)\n";
            let (host, map, urls) = setup(
                &[("lib.sail", declaration), ("main.sail", main)],
                &[0, 1],
                vec![
                    ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
                    ModuleData {
                        files: vec![FileId::from_raw(1)],
                        requires: if required { vec![0] } else { vec![] },
                    },
                ],
            );
            let analysis = snapshot(&host, &map);
            let position =
                analysis.file(&urls[1]).unwrap().position_at(main.find("helper").unwrap() + 1);
            let result = analysis.explain_position(FileId::from_raw(1), position).unwrap();
            let context = &result["contexts"][0];
            assert_eq!(context["resolved"], resolved, "{result}");
            assert_eq!(context["candidates"][0]["reason"], reason, "{result}");
            assert_eq!(context["moduleIndex"], 1, "{result}");
        }
    }

    #[test]
    fn distinguishes_local_bindings_and_inactive_regions() {
        let main =
            "function use(x : int) -> int = x\n$ifdef OFF\nval hidden : unit -> unit\n$endif\n";
        let (host, map, urls) = setup(&[("main.sail", main)], &[0], vec![]);
        let analysis = snapshot(&host, &map);
        for (needle, active, resolution) in [
            ("x\n", true, "local binding or type variable"),
            ("hidden", false, "inactive conditional source region"),
        ] {
            let position = analysis.file(&urls[0]).unwrap().position_at(main.find(needle).unwrap());
            let result = analysis.explain_position(FileId::from_raw(0), position).unwrap();
            assert_eq!(result["contexts"][0]["active"], active, "{result}");
            assert_eq!(result["contexts"][0]["resolution"], resolution, "{result}");
        }
    }
}
