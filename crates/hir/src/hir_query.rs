//! Salsa tracked queries for the HIR layer.
//!
//! Re-exports the per-file callgraph query from hir-def and adds
//! workspace-level queries on top.
//!
//! Created in stage , refactored in b.

use std::sync::Arc;

use base_db::FileText;
use hir_def::callgraph::WorkspaceCallGraph;

// Re-export the per-file callgraph query from hir-def so existing
// consumers of `hir::hir_query::callgraph` keep working.
pub use hir_def::def_query::{callgraph, ArcCallGraph};

/// Build the workspace-wide merged CallGraph from a set of FileTexts.
///
/// This is a regular function (not salsa-tracked) because it needs a
/// dynamic set of FileText inputs. The binary crate calls this when
/// it needs a workspace callgraph and manages its own caching.
pub fn build_workspace_callgraph(
    db: &dyn salsa::Database,
    file_texts: &[FileText],
) -> Arc<WorkspaceCallGraph> {
    let graphs: Vec<&hir_def::callgraph::CallGraph> = file_texts
        .iter()
        .filter_map(|ft| callgraph(db, *ft).as_ref().map(|acg| acg.0.as_ref()))
        .collect();

    Arc::new(WorkspaceCallGraph::from_callgraphs(graphs))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use base_db::FileId;

    #[salsa::db]
    #[derive(Default, Clone)]
    pub(crate) struct TestDb {
        storage: salsa::Storage<Self>,
    }

    #[salsa::db]
    impl salsa::Database for TestDb {}

    impl base_db::SourceDatabase for TestDb {
        fn file_text(&self, _: base_db::FileId) -> base_db::FileText {
            unimplemented!()
        }
        fn all_file_ids(&self) -> Vec<base_db::FileId> {
            unimplemented!()
        }
        fn set_file_text(&mut self, _: base_db::FileId, _: &str) {
            unimplemented!()
        }
        fn set_file_text_with_durability(
            &mut self,
            _: base_db::FileId,
            _: &str,
            _: base_db::Durability,
        ) {
            unimplemented!()
        }
        fn source_root(&self, _: base_db::SourceRootId) -> base_db::SourceRootInput {
            unimplemented!()
        }
        fn file_source_root(&self, _: base_db::FileId) -> base_db::FileSourceRootInput {
            unimplemented!()
        }
        fn set_file_source_root_with_durability(
            &mut self,
            _: base_db::FileId,
            _: base_db::SourceRootId,
            _: base_db::Durability,
        ) {
            unimplemented!()
        }
        fn set_source_root_with_durability(
            &mut self,
            _: base_db::SourceRootId,
            _: std::sync::Arc<base_db::SourceRoot>,
            _: base_db::Durability,
        ) {
            unimplemented!()
        }
    }

    #[test]
    fn callgraph_for_calling_functions() {
        let db = TestDb::default();
        let source = "function add(x : int, y : int) -> int = x + y\n\
                       function main() -> int = add(1, 2)\n";
        let input = FileText::new(&db, Arc::from(source), FileId::from_raw(0));

        let cg = callgraph(&db, input);
        assert!(cg.is_some(), "should build callgraph");
        let cg = &cg.as_ref().unwrap().0;
        assert!(cg.callees_of("main").any(|name| name == "add"), "main should call add");
    }

    #[test]
    fn callgraph_memoized() {
        let db = TestDb::default();
        let input = FileText::new(&db, Arc::from("function f() -> int = 1\n"), FileId::from_raw(0));

        let r1 = callgraph(&db, input);
        let r2 = callgraph(&db, input);
        match (r1.as_ref(), r2.as_ref()) {
            (Some(a), Some(b)) => assert!(Arc::ptr_eq(&a.0, &b.0), "callgraph should be memoized"),
            _ => panic!("both should be Some"),
        }
    }

    #[test]
    fn workspace_callgraph_cross_file() {
        let db = TestDb::default();
        let f0 = FileText::new(
            &db,
            Arc::from("function add(x : int, y : int) -> int = x + y\n"),
            FileId::from_raw(0),
        );
        let f1 = FileText::new(
            &db,
            Arc::from("function main() -> int = add(1, 2)\n"),
            FileId::from_raw(1),
        );

        let wscg = build_workspace_callgraph(&db, &[f0, f1]);
        assert!(wscg.has_any_caller("add"), "workspace callgraph: add should have callers");
        assert_eq!(
            wscg.site_count_to("add"),
            1,
            "workspace callgraph: add should have 1 call site"
        );
    }
}
