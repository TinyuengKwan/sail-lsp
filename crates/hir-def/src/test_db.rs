//! Test database for hir-def tests.
//! Provides a shared `TestDb` that implements `salsa::Database`,
//! `base_db::SourceDatabase`, and `DefDatabase` for use across test
//! modules in this crate.

#[cfg(test)]
mod imp {
    use std::sync::Arc;

    use base_db::{Durability, FileId, FileText, SourceRootId};

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
}
