//! Applying a batch of edits to the database in one revision.
//!
//! Mirrors rust-analyzer's `ide-db::apply_change`: a caller builds a
//! [`Change`] describing everything that moved, and one method applies it.
//! Spreading the steps out — buffer contents here, flush there, refresh the
//! workspace file list somewhere else — means every new call site has to
//! remember the whole sequence, and forgetting one is silent.

use base_db::{FileChange, FileId, SourceRoot};

use crate::root_database::RootDatabase;

/// Edits to apply together.
///
/// Wraps `base_db::FileChange`, which is where a change is turned into salsa
/// writes — rust-analyzer's `ChangeWithProcMacros` has the same shape and the
/// same reason: the durability of a file's text follows from the source root
/// it belongs to, so it is derived in one place rather than passed in by
/// every caller. This type used to carry a `Durability` per file, and the
/// three callers that built one each decided it differently.
#[derive(Default, Debug)]
pub struct Change {
    pub source_change: FileChange,
    /// The workspace's semantic file order. This cannot be reconstructed
    /// from the hash-based file store without losing project ordering.
    workspace_file_ids: Option<Vec<FileId>>,
    module_graph: Option<base_db::module_graph::ModuleGraph>,
    source_project: Option<base_db::preprocess::SourceProject>,
    strict_bitvector: Option<bool>,
}

impl Change {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue new contents for a file.
    pub fn change_file(&mut self, file_id: FileId, text: impl Into<String>) {
        self.source_change.change_file(file_id, Some(text.into()));
    }

    /// Replace the source roots. A file's text durability is read off the
    /// root it lands in, so this is also how a file is told apart from a
    /// standard-library one.
    pub fn set_roots(&mut self, roots: Vec<SourceRoot>) {
        self.source_change.set_roots(roots);
    }

    /// Replace the workspace file set in semantic order. Content edits alone
    /// do not need this: they reach `workspace_context` through `FileText` on
    /// their own.
    pub fn set_workspace_files(&mut self, file_ids: Vec<FileId>) {
        self.workspace_file_ids = Some(file_ids);
    }

    pub fn set_module_graph(&mut self, graph: base_db::module_graph::ModuleGraph) {
        self.module_graph = Some(graph);
    }

    pub fn set_source_project(&mut self, project: base_db::preprocess::SourceProject) {
        self.source_project = Some(project);
    }

    pub fn set_strict_bitvector(&mut self, enabled: bool) {
        self.strict_bitvector = Some(enabled);
    }

    pub fn is_empty(&self) -> bool {
        self.source_change.is_empty()
            && self.workspace_file_ids.is_none()
            && self.module_graph.is_none()
            && self.source_project.is_none()
            && self.strict_bitvector.is_none()
    }
}

impl RootDatabase {
    /// Apply a [`Change`] in one revision.
    ///
    /// Cancels in-flight queries first, so anything still running on the old
    /// revision unwinds through `salsa::Cancelled` rather than reading a
    /// half-updated database. Returns whether anything changed.
    pub fn apply_change(&mut self, change: Change) -> bool {
        if change.is_empty() {
            return false;
        }
        use salsa::Database as _;
        let cancel_started = std::time::Instant::now();
        self.trigger_cancellation();
        let cancel_us = cancel_started.elapsed().as_micros();
        let inputs_started = std::time::Instant::now();

        if let Some(enabled) = change.strict_bitvector {
            use salsa::Setter;
            if let Some(options) = base_db::LanguageOptions::try_get(self) {
                if options.strict_bitvector(self) != enabled {
                    options.set_strict_bitvector(self).to(enabled);
                }
            }
        }
        let anything_written = !change.source_change.is_empty();
        change.source_change.apply(self);
        // A change happened, and the nonce is how the server's main loop
        // knows. `bump_nonce` had no caller, so the nonce never moved
        // after construction: the loop's `current_nonce !=
        // last_gc_revision` was true once, ran `trigger_lru_eviction`
        // once, and was false for the rest of the session. The two
        // LRU-capped queries kept everything they had ever computed.
        self.bump_nonce();

        let graph_changed = change.module_graph.is_some();
        if let Some(graph) = change.module_graph {
            base_db::module_graph::set_module_graph(self, graph);
        }
        let membership_changed =
            change.workspace_file_ids.is_some() || change.source_project.is_some();
        if let Some(project) = change.source_project {
            use base_db::preprocess::WorkspaceSourceProject;
            use salsa::Setter;
            let files = project
                .paths
                .iter()
                .filter_map(|(_, id)| self.files().file_text(*id).map(|ft| (*id, ft)))
                .collect();
            if let Some(input) = WorkspaceSourceProject::try_get(self) {
                if input.project(self) != &project {
                    input.set_project(self).to(project);
                }
                if input.files(self) != &files {
                    input.set_files(self).to(files);
                }
            } else {
                WorkspaceSourceProject::new(self, project, files);
            }
        }
        let inputs_us = inputs_started.elapsed().as_micros();
        let plan_started = std::time::Instant::now();
        if let Some(project) = base_db::preprocess::WorkspaceSourceProject::try_get(self) {
            let plan = hir::preprocess::workspace_plan(self, project).clone();
            let mut regions = plan.regions;
            let mut sources = std::collections::BTreeMap::new();
            let mut instances: std::collections::BTreeMap<_, Vec<_>> =
                std::collections::BTreeMap::new();
            let mut graph = plan.modules;
            for module in &mut graph.modules {
                module.files.clear();
            }
            graph.globals.clear();
            let mut file_texts = Vec::new();
            let mut instance_ids = std::collections::HashMap::new();
            for instance in plan.instances {
                if !instance.primary && !instance.active {
                    continue;
                }
                let Some(source) = self.files().file_text(instance.source) else { continue };
                let file = if instance.primary {
                    source
                } else {
                    self.include_instance(instance.key.clone(), source)
                };
                let id = file.file_id(self);
                instance_ids.insert(instance.key.clone(), id);
                if id.is_instance() {
                    sources.insert(id, source);
                }
                regions.insert(id, instance.regions);
                instances.entry(instance.source).or_default().push(id);
                match instance.key.module {
                    Some(owner) => graph.modules[owner].files.push(id),
                    None => graph.globals.push(id),
                }
                file_texts.push(file);
            }
            // Primary source view first, independently of postorder traversal
            // (a guarded recursive visit can finish before its outer visit).
            for (source, views) in &mut instances {
                views.sort_by_key(|id| (*id != *source, id.index()));
            }
            let order = plan
                .order
                .into_iter()
                .filter_map(|(key, region)| {
                    let id = *instance_ids.get(&key)?;
                    regions.get(&id)?.get(region).copied().unwrap_or(false).then_some((id, region))
                })
                .collect();
            base_db::preprocess::set_preprocessing(self, regions, sources, instances, order);
            base_db::module_graph::set_module_graph(self, graph);
            hir::def_query::set_workspace_files(self, file_texts);
        } else if let Some(file_ids) = change.workspace_file_ids {
            let file_texts: Vec<base_db::FileText> =
                file_ids.into_iter().filter_map(|fid| self.files().file_text(fid)).collect();
            hir::def_query::set_workspace_files(self, file_texts);
        }
        tracing::debug!(target: "sail_lsp::latency",
            "apply_change cancel_us={cancel_us} inputs_us={inputs_us} plan_us={}",
            plan_started.elapsed().as_micros());
        anything_written || graph_changed || membership_changed
    }
}

#[cfg(test)]
mod tests {
    use super::Change;
    use crate::root_database::RootDatabase;

    #[test]
    fn source_order_interleaves_include_regions_and_keeps_instance_identity() {
        use base_db::{
            preprocess::{SourceProject, WorkspacePreprocessing},
            FileId,
        };
        let ids: Vec<_> = (0..3).map(FileId::from_raw).collect();
        let mut db = RootDatabase::default();
        let mut project = SourceProject {
            roots: vec![ids[0], ids[0]],
            paths: ["/main.sail", "/middle.sail", "/leaf.sail"]
                .into_iter()
                .zip(ids.iter().copied())
                .map(|(p, id)| (p.into(), id))
                .collect(),
            ..Default::default()
        };
        let main =
            "val before : int\n$include \"middle.sail\"\nval after : int\nfunction body() = 1\n";
        let mut change = Change::new();
        change.change_file(ids[0], main);
        change.change_file(ids[1], "val middle : int\n$include \"leaf.sail\"\nval tail : int\n");
        change.change_file(ids[2], "val leaf : int\n");
        change.set_source_project(project.clone());
        db.apply_change(change);
        let input = WorkspacePreprocessing::try_get(&db).unwrap();
        let order = input.order(&db).clone();
        let declarations = hir::source_order::declarations(&db).unwrap().clone();
        let labels: Vec<_> = declarations
            .iter()
            .filter_map(|&d| {
                let node = hir::source_order::node(&db, d)?;
                (node.kind() != parser::SyntaxKind::DIRECTIVE_DEF).then(|| node.text().to_string())
            })
            .collect();
        assert_eq!(
            &labels[..6],
            [
                "val before : int",
                "val middle : int",
                "val leaf : int",
                "val tail : int",
                "val after : int",
                "function body() = 1"
            ]
        );
        let physical: Vec<_> = order
            .iter()
            .map(|&(id, region)| (base_db::preprocess::physical_file(&db, id), region))
            .collect();
        let once = vec![(ids[0], 0), (ids[1], 0), (ids[2], 0), (ids[1], 1), (ids[0], 1)];
        assert_eq!(physical, once.repeat(2));
        for (first, second) in order[..5].iter().zip(&order[5..]) {
            assert_ne!(first.0, second.0, "each expansion has its own semantic identity");
        }
        let mut change = Change::new();
        change.change_file(ids[0], main.replace("= 1", "= {\n  12345\n}"));
        db.apply_change(change);
        assert_eq!(input.order(&db), &order, "body edits do not change declaration order inputs");
        assert_eq!(hir::source_order::declarations(&db).unwrap(), &declarations);
        project.roots.pop();
        let mut change = Change::new();
        change.set_source_project(project);
        db.apply_change(change);
        assert_eq!(input.order(&db), &order[..5]);
    }

    #[test]
    fn include_instances_isolate_branches_owners_and_inference() {
        use base_db::{
            module_graph::{ModuleData, ModuleGraph},
            preprocess::SourceProject,
            FileId,
        };
        let ids: Vec<_> = (0..3).map(FileId::from_raw).collect();
        let shared = "$ifdef SECOND\nprivate val second : unit -> bool\nfunction second() = helper_b()\n$else\nprivate val first : unit -> int\nfunction first() = helper_a()\n$endif\n";
        let mut db = RootDatabase::default();
        let mut change = Change::new();
        change.change_file(ids[0], "private val helper_a : unit -> int\nfunction helper_a() = 1\n$include \"shared.sail\"\nval use_a : unit -> int\nfunction use_a() = first()\n");
        change.change_file(ids[1], "$define SECOND\nprivate val helper_b : unit -> bool\nfunction helper_b() = true\n$include \"shared.sail\"\nval use_b : unit -> bool\nfunction use_b() = second()\n");
        change.change_file(ids[2], shared);
        change.set_source_project(SourceProject {
            roots: ids[..2].to_vec(),
            paths: ["/a.sail", "/b.sail", "/shared.sail"]
                .into_iter()
                .zip(ids.iter().copied())
                .map(|(p, id)| (p.into(), id))
                .collect(),
            modules: ModuleGraph {
                modules: ids[..2]
                    .iter()
                    .map(|&id| ModuleData { files: vec![id], requires: vec![] })
                    .collect(),
                globals: vec![],
            },
            ..Default::default()
        });
        db.apply_change(change);
        let views = base_db::preprocess::file_instances(&db, ids[2]);
        assert_eq!(views.len(), 2);
        assert_eq!(views[0], ids[2]);
        assert!(views[1].is_instance());
        assert_eq!(db.files().all_file_ids(), ids, "semantic views are not VFS members");
        let a = db.files().file_text(views[0]).unwrap();
        let b = db.files().file_text(views[1]).unwrap();
        assert!(std::sync::Arc::ptr_eq(a.text(&db), b.text(&db)), "text is shared");
        let raw_a = syntax::parse_query::parse_file(&db, a);
        let raw_b = syntax::parse_query::parse_file(&db, b);
        assert!(std::sync::Arc::ptr_eq(&raw_a.tokens, &raw_b.tokens), "raw parsing is shared");
        for (caller, own, foreign, file) in
            [(ids[0], "first", "second", a), (ids[1], "second", "first", b)]
        {
            assert!(hir::visibility::symbol_visible_from(&db, caller, file, own));
            let tree = hir::def_query::file_item_tree(&db, file).as_ref().unwrap();
            let names: Vec<_> =
                tree.top_level_items().iter().map(|id| id.name(tree).as_str()).collect();
            assert!(names.contains(&own));
            assert!(!names.contains(&foreign));
            let other = if caller == ids[0] { ids[1] } else { ids[0] };
            assert!(!hir::visibility::symbol_visible_from(&db, other, file, own));
        }
        let ws = base_db::WorkspaceFiles::try_get(&db).unwrap();
        for &ft in ws.file_texts(&db) {
            for &body in hir::def_query::file_def_with_body_ids(&db, ft) {
                assert!(hir::ty_query::infer(&db, body).0.inference_diagnostics().is_empty());
            }
        }
        let body_b = hir::def_query::file_def_with_body_ids(&db, b)[0];
        let before = hir::ty_query::infer(&db, body_b).0.clone();
        let mut change = Change::new();
        change.change_file(ids[2], shared.replace("helper_a()", "2"));
        db.apply_change(change);
        assert_eq!(views, base_db::preprocess::file_instances(&db, ids[2]));
        let body_b = hir::def_query::file_def_with_body_ids(&db, b)[0];
        assert!(
            std::sync::Arc::ptr_eq(&before, &hir::ty_query::infer(&db, body_b).0),
            "editing one branch preserves the other instance's inference"
        );
        let original_b = db.files().file_text(ids[1]).unwrap().text(&db).to_string();
        let mut change = Change::new();
        change.change_file(ids[1], "val detached : unit -> bool\nfunction detached() = true\n");
        db.apply_change(change);
        assert_eq!(base_db::preprocess::file_instances(&db, ids[2]), [ids[2]]);
        assert!(!base_db::WorkspaceFiles::try_get(&db).unwrap().file_texts(&db).contains(&b));
        let mut change = Change::new();
        change.change_file(ids[1], original_b);
        db.apply_change(change);
        assert_eq!(
            base_db::preprocess::file_instances(&db, ids[2]),
            views,
            "returning include reuses its identity"
        );
    }

    #[test]
    fn preprocessing_follows_includes_order_target_and_body_invalidation() {
        use base_db::{
            module_graph::{ModuleData, ModuleGraph},
            preprocess::SourceProject,
            FileId,
        };
        let ids: Vec<_> = (0..5).map(FileId::from_raw).collect();
        let sources = [
            "$ifdef LATER\nval too_early : int\n$endif\n$target_set native c ocaml\ndefault Order dec\n$include <select.sail> // comment\n$ifdef FROM_INCLUDE\nval after_include : int\n$endif\n$ifdef NEVER\n$include <missing.sail>\nfunction inactive() = absent()\n$endif\n",
            "$define LATER\nval consumer : unit -> int\nfunction consumer() = selected()\n",
            "$ifndef GUARD\n$define GUARD\n$iftarget native\n$include \"c.sail\"\n$else\n$include \"fallback.sail\"\n$endif\n$define FROM_INCLUDE\n$endif\n",
            "val selected : unit -> int\nfunction selected() = 1\n",
            "val selected : unit -> int\nfunction selected() = 2\n",
        ];
        let mut project = SourceProject {
            roots: ids[..2].to_vec(),
            paths: [
                "/project/start.sail",
                "/project/main.sail",
                "/lib/select.sail",
                "/lib/c.sail",
                "/lib/fallback.sail",
            ]
            .into_iter()
            .zip(ids.iter().copied())
            .map(|(p, id)| (p.into(), id))
            .collect(),
            library: Some("/lib".into()),
            modules: ModuleGraph {
                modules: vec![
                    ModuleData { files: vec![ids[0]], requires: vec![] },
                    ModuleData { files: vec![ids[1]], requires: vec![0] },
                ],
                globals: vec![],
            },
            ..Default::default()
        };
        let mut db = RootDatabase::default();
        let mut change = Change::new();
        for (&id, text) in ids.iter().zip(sources) {
            change.change_file(id, text);
        }
        change.set_source_project(project.clone());
        db.apply_change(change);
        for target in [None, Some("c"), None] {
            project.target = target.map(str::to_owned);
            let mut change = Change::new();
            change.set_source_project(project.clone());
            db.apply_change(change);
            let input = base_db::preprocess::WorkspaceSourceProject::try_get(&db).unwrap();
            let plan = hir::preprocess::workspace_plan(&db, input);
            assert!(plan.missing.is_empty(), "inactive missing include: {:?}", plan.missing);
            let chosen = if target.is_some() { ids[3] } else { ids[4] };
            let excluded = if target.is_some() { ids[4] } else { ids[3] };
            assert!(plan.files.contains(&chosen));
            assert!(!plan.files.contains(&excluded));
            assert!(plan.modules.modules[0].files.contains(&chosen), "include inherits its module");
            let ft = db.files().file_text(ids[0]).unwrap();
            let tree = hir::def_query::file_item_tree(&db, ft).as_ref().unwrap();
            let names: Vec<_> =
                tree.top_level_items().iter().map(|id| id.name(tree).as_str()).collect();
            assert!(names.contains(&"after_include"));
            assert!(!names.contains(&"too_early"));
            assert!(hir::def_query::file_def_with_body_ids(&db, ft).is_empty());
            let caller =
                hir::def_query::file_def_with_body_ids(&db, db.files().file_text(ids[1]).unwrap())
                    [0];
            let before = hir::ty_query::infer(&db, caller).0.clone();
            assert!(
                before.inference_diagnostics().is_empty(),
                "{:?}",
                before.inference_diagnostics()
            );
            let mut change = Change::new();
            change.change_file(chosen, "val selected : unit -> int\nfunction selected() = 3\n");
            db.apply_change(change);
            let caller =
                hir::def_query::file_def_with_body_ids(&db, db.files().file_text(ids[1]).unwrap())
                    [0];
            assert!(
                std::sync::Arc::ptr_eq(&before, &hir::ty_query::infer(&db, caller).0),
                "body edit preserves caller inference"
            );
        }
    }

    #[test]
    fn guarded_recursion_terminates_and_real_include_cycles_are_recorded() {
        use base_db::{preprocess::SourceProject, FileId};
        let id = FileId::from_raw(0);
        let mut db = RootDatabase::default();
        let mut change = Change::new();
        change.change_file(
            id,
            "$ifndef ONCE\n$define ONCE\n$include \"self.sail\"\nval present : int\n$endif\n",
        );
        change.set_source_project(SourceProject {
            roots: vec![id],
            paths: vec![("/self.sail".into(), id)],
            ..Default::default()
        });
        db.apply_change(change);
        let input = base_db::preprocess::WorkspaceSourceProject::try_get(&db).unwrap();
        assert!(hir::preprocess::workspace_plan(&db, input).cycles.is_empty());
        assert!(hir::preprocess::workspace_plan(&db, input).edges.is_empty());
        let mut change = Change::new();
        change.change_file(id, "$include \"self.sail\"\nval present : int\n");
        db.apply_change(change);
        assert_eq!(hir::preprocess::workspace_plan(&db, input).cycles, [id]);
        let mut project = input.project(&db).clone();
        project.target = Some("c".into());
        let mut change = Change::new();
        change.set_source_project(project);
        change.change_file(
            id,
            "$iftarget stop\n$else\n$target_set stop c\n$include \"self.sail\"\n$endif\n",
        );
        db.apply_change(change);
        assert!(
            hir::preprocess::workspace_plan(&db, input).cycles.is_empty(),
            "target-set state can terminate recursion too"
        );
    }

    /// A change moves the nonce.
    ///
    /// The server's main loop schedules `trigger_lru_eviction` off this
    /// number — `if current_nonce != last_gc_revision` — and `bump_nonce`
    /// had no caller, so it moved once at construction and never again.
    /// Eviction ran at startup and the two LRU-capped queries kept
    /// everything they computed for the rest of the session.
    #[test]
    fn a_change_moves_the_nonce() {
        let mut db = RootDatabase::default();
        let before = db.nonce();

        let mut change = Change::default();
        change.change_file(base_db::FileId::from_raw(0), "val f : int -> int\n");
        assert!(db.apply_change(change), "the change was applied");
        let after = db.nonce();
        assert_ne!(before, after, "the nonce moved");

        // And again, so the loop sees every change and not just the first.
        let mut change = Change::default();
        change.change_file(base_db::FileId::from_raw(0), "val g : int -> int\n");
        assert!(db.apply_change(change));
        assert_ne!(after, db.nonce(), "the nonce moves on every change");
    }

    /// An empty change is not a change.
    #[test]
    fn an_empty_change_leaves_the_nonce_alone() {
        let mut db = RootDatabase::default();
        let before = db.nonce();
        assert!(!db.apply_change(Change::default()));
        assert_eq!(before, db.nonce());
    }

    #[test]
    fn workspace_files_follow_the_explicit_semantic_order() {
        let mut db = RootDatabase::default();
        let mut change = Change::default();
        for raw in [2, 0, 1] {
            change.change_file(base_db::FileId::from_raw(raw), format!("val value_{raw} : int\n"));
        }
        change.set_workspace_files(vec![
            base_db::FileId::from_raw(1),
            base_db::FileId::from_raw(2),
            base_db::FileId::from_raw(0),
        ]);
        assert!(db.apply_change(change));

        let workspace = base_db::WorkspaceFiles::try_get(&db).expect("workspace input");
        let ids: Vec<_> =
            workspace.file_texts(&db).iter().map(|file| file.file_id(&db).index()).collect();
        assert_eq!(ids, [1, 2, 0]);
    }
}
