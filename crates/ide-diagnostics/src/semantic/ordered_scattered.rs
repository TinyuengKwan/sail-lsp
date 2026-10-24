//! Render shared declaration-order scattered findings as editor diagnostics.
#[cfg(test)]
use base_db::FileId;
use base_db::{FileText, WorkspaceFiles};

pub(crate) fn diagnostics(
    db: &dyn salsa::Database,
    file: FileText,
) -> Option<Vec<crate::Diagnostic>> {
    let program = hir::source_order::program(db, file)?;
    let workspace = WorkspaceFiles::try_get(db)?;
    Some(
        hir::source_order::scattered::issues(db, workspace, program)
            .iter()
            .filter(|i| i.file == file.file_id(db))
            .map(|i| {
                crate::Diagnostic::new(
                    hir::diagnostics::DiagnosticCode::SailError(i.code),
                    i.message.clone(),
                    hir::FileRange { file_id: i.file, range: i.range },
                )
                .with_related(
                    i.related
                        .iter()
                        .map(|(file, range, message)| {
                            (hir::FileRange { file_id: *file, range: *range }, message.clone())
                        })
                        .collect(),
                )
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_db::{apply_change::Change, root_database::RootDatabase};
    fn fixture(sources: &[(&str, &str)], roots: &[usize]) -> (RootDatabase, Vec<FileText>) {
        let mut db = RootDatabase::default();
        let ids: Vec<_> = (0..sources.len() as u32).map(FileId::from_raw).collect();
        let mut change = Change::new();
        for (&id, (_, text)) in ids.iter().zip(sources) {
            change.change_file(id, *text);
        }
        change.set_source_project(base_db::preprocess::SourceProject {
            roots: roots.iter().map(|&i| ids[i]).collect(),
            paths: sources
                .iter()
                .zip(&ids)
                .map(|((name, _), &id)| (format!("/project/{name}").into(), id))
                .collect(),
            ..Default::default()
        });
        db.apply_change(change);
        let files = ids.iter().map(|&id| db.files().file_text(id).unwrap()).collect();
        (db, files)
    }
    fn codes(db: &RootDatabase, file: FileText) -> Vec<String> {
        diagnostics(db, file).unwrap().iter().map(|d| d.code.as_str().to_owned()).collect()
    }
    #[test]
    fn inline_scattered_scope_uses_declaration_owners() {
        for (private, requires, rejected) in
            [(false, true, false), (false, false, true), (true, true, true)]
        {
            let text = format!(
                "$project# A {{}} B {{ {} }}\n$start_module# A\n{}scattered union U\nunion clause U = C : int\n$end_module#\n$start_module# B\nunion clause U = D : int\nend U\n$end_module#\n",
                if requires { "requires A" } else { "" },
                if private { "private " } else { "" },
            );
            let mut db = RootDatabase::default();
            let id = FileId::from_raw(0);
            let mut change = Change::new();
            change.change_file(id, &text);
            change.set_workspace_files(vec![id]);
            db.apply_change(change);
            let file = db.files().file_text(id).unwrap();
            let findings = diagnostics(&db, file).unwrap();
            assert_eq!(findings.len(), usize::from(rejected), "{findings:?}");
            if rejected {
                assert_eq!(findings[0].code.as_str(), "scattered-not-visible");
                let clause = text.find("U = D").unwrap();
                let head = text.find("union U").unwrap() + 6;
                assert_eq!(findings[0].range.range, base_db::text_range(clause, clause + 1));
                assert_eq!(findings[0].related[0].0.file_id, id);
                assert_eq!(findings[0].related[0].0.range, base_db::text_range(head, head + 1));
            }
        }
    }

    #[test]
    fn duplicate_members_report_the_original_member_location() {
        for (head, member, expected) in [
            ("scattered union U\n", "union clause U = C : int\n", "C"),
            ("scattered enum U\n", "enum clause U = C\n", "U"),
        ] {
            let first = format!("{head}{member}");
            let (db, files) = fixture(&[("first.sail", &first), ("second.sail", member)], &[0, 1]);
            let found = diagnostics(&db, files[1]).unwrap();
            assert_eq!(found.len(), 1, "{found:?}");
            assert_eq!(found[0].code.as_str(), "duplicate-scattered-member");
            let at = member.find(expected).unwrap();
            assert_eq!(found[0].range.range, base_db::text_range(at, at + 1));
            let previous = first.find("C").unwrap();
            assert_eq!(found[0].related[0].0.file_id, files[0].file_id(&db));
            assert_eq!(found[0].related[0].0.range, base_db::text_range(previous, previous + 1));
        }
    }
    #[test]
    fn include_is_interleaved_with_parent_head_and_end() {
        let (mut db, files) = fixture(
            &[
                ("main.sail", "scattered union U\n$include \"clause.sail\"\nend U\n"),
                ("clause.sail", "union clause U = C : int\n"),
            ],
            &[0],
        );
        assert!(codes(&db, files[0]).is_empty());
        assert!(codes(&db, files[1]).is_empty());
        let mut change = Change::new();
        change.change_file(
            files[0].file_id(&db),
            "scattered union U\nend U\n$include \"clause.sail\"\n",
        );
        db.apply_change(change);
        assert!(codes(&db, files[1]).iter().any(|c| c == "scattered-clause-after-end"));
        let found = diagnostics(&db, files[1]).unwrap();
        assert_eq!(found[0].related[0].0.file_id, files[0].file_id(&db));
    }
    #[test]
    fn future_heads_and_second_ends_are_rejected() {
        let (db, files) = fixture(
            &[
                ("first.sail", "union clause U = Early : int\nend U\n"),
                ("later.sail", "scattered union U\nunion clause U = C : int\nend U\n"),
                ("last.sail", "end U\n"),
            ],
            &[0, 1, 2],
        );
        assert_eq!(codes(&db, files[0]), ["scattered-missing-head", "invalid-scattered-end"]);
        assert_eq!(codes(&db, files[2]), ["scattered-double-end"]);
    }
    #[test]
    fn callable_clauses_open_groups_only_after_a_signature() {
        let (db, files) = fixture(
            &[
                ("sig.sail", "val f : unit -> int\n"),
                ("clause.sail", "function clause f() = 1\n"),
                ("end.sail", "end f\n"),
            ],
            &[0, 1, 2],
        );
        assert!(files.iter().all(|&f| codes(&db, f).is_empty()));
        let (db, files) = fixture(
            &[("clause.sail", "function clause f() = 1\n"), ("sig.sail", "val f : unit -> int\n")],
            &[0, 1],
        );
        assert_eq!(codes(&db, files[0]), ["scattered-missing-signature"]);
        let (db, files) = fixture(
            &[
                ("sig.sail", "private val f : unit -> int\n"),
                ("clause.sail", "function clause f() = 1\nend f\n"),
            ],
            &[0, 1],
        );
        assert!(files.iter().all(|&f| codes(&db, f).is_empty()));
    }
    #[test]
    fn union_payload_retains_the_cross_file_head_environment() {
        let (db, files) = fixture(
            &[
                ("head.sail", "scattered union U\n"),
                ("clause.sail", "union clause U = C : U\nend U\n"),
            ],
            &[0, 1],
        );
        assert_eq!(codes(&db, files[1]), ["scattered-forward-type"]);
        for (roots, rejected) in [(vec![0, 1], true), (vec![1, 0], false)] {
            let (db, files) = fixture(
                &[
                    ("main.sail", "scattered union U\n$include \"clause.sail\"\n"),
                    ("type.sail", "type later = int\n"),
                    ("clause.sail", "union clause U = C : later\n"),
                ],
                &roots,
            );
            // Both a type after the clause and one between head and clause
            // are unavailable in the retained head environment.
            if rejected {
                assert_eq!(codes(&db, files[2]), ["scattered-forward-type"]);
                let mut db = db;
                let mut change = Change::new();
                change.change_file(
                    files[0].file_id(&db),
                    "scattered union U\n$include \"type.sail\"\n$include \"clause.sail\"\n",
                );
                db.apply_change(change);
                assert_eq!(codes(&db, files[2]), ["scattered-forward-type"]);
            } else {
                assert!(codes(&db, files[2]).is_empty());
            }
        }
    }
    #[test]
    fn repeated_include_end_uses_semantic_instances() {
        let (db, files) = fixture(&[
            ("main.sail", "scattered union U\nunion clause U = C : int\n$include \"end.sail\"\n$include \"end.sail\"\n"),
            ("end.sail", "end U\n"),
        ], &[0]);
        let views = base_db::preprocess::file_instances(&db, files[1].file_id(&db));
        assert_eq!(views.len(), 2);
        assert!(codes(&db, files[1]).is_empty());
        assert_eq!(codes(&db, db.files().file_text(views[1]).unwrap()), ["scattered-double-end"]);
    }
    #[test]
    fn union_payload_uses_clause_module_with_head_global_bindings() {
        use base_db::{
            module_graph::{ModuleData, ModuleGraph},
            preprocess::WorkspaceSourceProject,
        };
        for (requires_type, private) in [(true, false), (false, false), (true, true)] {
            let text = if private { "private type T = int\n" } else { "type T = int\n" };
            let (mut db, files) = fixture(
                &[
                    ("type.sail", text),
                    ("head.sail", "scattered union U\n"),
                    ("clause.sail", "union clause U = C : T\nend U\n"),
                ],
                &[0, 1, 2],
            );
            let mut project = WorkspaceSourceProject::try_get(&db).unwrap().project(&db).clone();
            project.modules = ModuleGraph {
                modules: vec![
                    ModuleData { files: vec![files[0].file_id(&db)], requires: vec![] },
                    ModuleData { files: vec![files[1].file_id(&db)], requires: vec![] },
                    ModuleData {
                        files: vec![files[2].file_id(&db)],
                        requires: if requires_type { vec![0, 1] } else { vec![1] },
                    },
                ],
                globals: vec![],
            };
            let mut change = Change::new();
            change.set_source_project(project);
            db.apply_change(change);
            let findings = codes(&db, files[2]);
            if requires_type && !private {
                assert!(findings.is_empty(), "{findings:?}");
            } else {
                assert_eq!(findings, ["scattered-forward-type"]);
            }
        }
    }

    #[test]
    fn extension_privacy_is_separate_from_global_end_state() {
        use base_db::{
            module_graph::{ModuleData, ModuleGraph},
            preprocess::WorkspaceSourceProject,
        };
        for (kind, clause) in [("union", "union clause U = C : int"), ("enum", "enum clause U = C")]
        {
            let head = format!("private scattered {kind} U\n{clause}\n");
            let extra = if kind == "union" {
                "union clause U = D : bool\nend U\n"
            } else {
                "enum clause U = D\nend U\n"
            };
            let (mut db, files) = fixture(&[("a.sail", &head), ("b.sail", extra)], &[0, 1]);
            let input = WorkspaceSourceProject::try_get(&db).unwrap();
            let mut project = input.project(&db).clone();
            project.modules = ModuleGraph {
                modules: vec![
                    ModuleData { files: vec![files[0].file_id(&db)], requires: vec![] },
                    ModuleData { files: vec![files[1].file_id(&db)], requires: vec![0] },
                ],
                globals: vec![],
            };
            let mut change = Change::new();
            change.set_source_project(project);
            db.apply_change(change);
            let findings = codes(&db, files[1]);
            assert_eq!(
                findings.iter().filter(|c| *c == "scattered-not-visible").count(),
                if kind == "enum" { 2 } else { 1 }
            );
            assert!(!findings.iter().any(|c| c == "invalid-scattered-end"));
        }
    }

    #[test]
    fn enum_function_and_mapping_clauses_observe_cross_file_end() {
        for (head, first, later) in [
            ("scattered enum E", "enum clause E = A\nend E", "enum clause E = B"),
            (
                "val f : int -> int\nscattered function f",
                "function clause f(0) = 1\nend f",
                "function clause f(_) = 2",
            ),
            (
                "val m : int <-> bool\nscattered mapping m",
                "mapping clause m = 0 <-> false\nend m",
                "mapping clause m = 1 <-> true",
            ),
        ] {
            let (db, files) = fixture(
                &[("head.sail", head), ("first.sail", first), ("later.sail", later)],
                &[0, 1, 2],
            );
            assert!(codes(&db, files[0]).is_empty());
            assert!(codes(&db, files[1]).is_empty());
            assert_eq!(codes(&db, files[2]), ["scattered-clause-after-end"]);
        }
    }
}
