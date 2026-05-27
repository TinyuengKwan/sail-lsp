//! Test database for hir-def tests.
//! Provides a shared `TestDb` that implements `salsa::Database`,
//! `base_db::SourceDatabase`, and `DefDatabase` for use across test
//! modules in this crate.

#[cfg(test)]
mod imp {
    use std::sync::Arc;

    use base_db::{Durability, FileId, FileText, SourceRootId};

    use crate::db::DefDatabase;
    use crate::item_tree::ItemTree;
    use crate::nameres::DefMap;

    #[salsa::db]
    #[derive(Default, Clone)]
    pub(crate) struct TestDb {
        storage: salsa::Storage<Self>,
    }

    #[salsa::db]
    impl salsa::Database for TestDb {}

    impl base_db::SourceDatabase for TestDb {
        fn file_text(&self, _: FileId) -> FileText {
            unimplemented!()
        }
        fn all_file_ids(&self) -> Vec<FileId> {
            unimplemented!()
        }
        fn set_file_text(&mut self, _: FileId, _: &str) {
            unimplemented!()
        }
        fn set_file_text_with_durability(&mut self, _: FileId, _: &str, _: Durability) {
            unimplemented!()
        }
        fn source_root(&self, _: SourceRootId) -> base_db::SourceRootInput {
            unimplemented!()
        }
        fn file_source_root(&self, _: FileId) -> base_db::FileSourceRootInput {
            unimplemented!()
        }
        fn set_file_source_root_with_durability(
            &mut self,
            _: FileId,
            _: SourceRootId,
            _: Durability,
        ) {
            unimplemented!()
        }
        fn set_source_root_with_durability(
            &mut self,
            _: SourceRootId,
            _: Arc<base_db::SourceRoot>,
            _: Durability,
        ) {
            unimplemented!()
        }
    }

    impl hir_expand::db::ExpandDatabase for TestDb {
        fn include_paths(&self, input: FileText) -> &[String] {
            crate::def_query::include_paths(self, input)
        }
    }

    impl DefDatabase for TestDb {
        fn file_item_tree(&self, input: FileText) -> Option<&Arc<ItemTree>> {
            crate::def_query::file_item_tree(self, input).as_ref()
        }
        fn callable_bodies(&self, input: FileText) -> Option<&crate::bodies::CallableBodies> {
            crate::def_query::callable_bodies(self, input).as_ref().map(|b| b.0.as_ref())
        }
        fn def_map(&self, input: FileText) -> Option<&DefMap> {
            crate::def_query::crate_def_map(self, input).as_ref().map(|d| d.0.as_ref())
        }
        fn callgraph(&self, input: FileText) -> Option<&crate::callgraph::CallGraph> {
            crate::def_query::callgraph(self, input).as_ref().map(|cg| cg.0.as_ref())
        }
        fn file_def_with_body_ids<'db>(
            &'db self,
            input: FileText,
        ) -> &'db [crate::def_query::DefWithBodyId<'db>] {
            crate::def_query::file_def_with_body_ids(self, input)
        }
        fn body_with_source_map<'db>(
            &'db self,
            id: crate::def_query::DefWithBodyId<'db>,
        ) -> &'db crate::def_query::ArcBodyWithSourceMap {
            crate::def_query::body_with_source_map(self, id)
        }
    }
}
