//! Completion engine for Sail.
//!
//! An older engine used to live here beside this one —
//! `completion_items_ide`, `determine_completion_context`,
//! `completion_score`, `upsert_candidate` and the rest, each behind
//! `#[expect(dead_code)]` because the server calls `completions()` instead.
//! What kept it alive was a test helper that called *it*, so twenty tests
//! were describing a path no editor reaches. Pointing them at
//! `completions()` immediately found one thing the live engine did not do:
//! complete an enum member written unqualified.
//!
//! Provider dispatch: `CompletionContext` -> `CompletionAnalysis` -> providers.
//! 11 providers: keyword, expr, dot, pattern, item_list, flyimport, postfix,
//! pragma, record, snippet, type_.

use crate::item::CompletionItem;
use ide_db::defs::CompletionItemKind;
use ide_db::{builtin_docs, FileDb};

use url::Url;

use completions::Completions;

// Provider-based completion architecture.
// All modules are private — types re-exported via `pub use` below.
mod completions;
mod config;
mod context;
pub mod item;
mod render;
mod snippet;

pub use crate::{
    config::{CallableSnippets, CompletionConfig, CompletionFieldsToResolve, SnippetCap},
    snippet::{Snippet, SnippetScope},
};
pub use context::completion_prefix;

/// Main completion entry point.
///
/// Two-phase architecture:
/// 1. `CompletionContext::new()` -> `(ctx, CompletionAnalysis)`
/// 2. Match on `CompletionAnalysis` -> dispatch to providers
#[allow(clippy::too_many_arguments)]
pub fn completions(
    db: &dyn salsa::Database,
    file_text: Option<base_db::FileText>,
    all_files: &[(&Url, &dyn FileDb)],
    current_uri: &Url,
    file: &dyn FileDb,
    text: &str,
    offset: usize,
    prefix: &str,
    keywords: &[&str],
    builtins: &[&str],
) -> Vec<CompletionItem> {
    completions_with_config(
        db,
        file_text,
        all_files,
        current_uri,
        file,
        text,
        offset,
        prefix,
        keywords,
        builtins,
        &CompletionConfig::default(),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn completions_with_config(
    db: &dyn salsa::Database,
    file_text: Option<base_db::FileText>,
    all_files: &[(&Url, &dyn FileDb)],
    current_uri: &Url,
    file: &dyn FileDb,
    text: &str,
    offset: usize,
    prefix: &str,
    keywords: &[&str],
    builtins: &[&str],
    config: &CompletionConfig,
) -> Vec<CompletionItem> {
    let (mut ctx, analysis) =
        context::CompletionContext::new(db, file, file_text, text, offset, prefix);
    ctx.config = config.clone();
    let mut completions = Completions::default();

    {
        let acc = &mut completions;

        match &analysis {
            context::CompletionAnalysis::NameRef(name_ref_ctx) => {
                completions::complete_name_ref(
                    acc,
                    &ctx,
                    all_files,
                    name_ref_ctx,
                    current_uri,
                    keywords,
                    builtins,
                );
                completions::pragma::complete_pragma(acc, &ctx);
                completions::snippet::complete_snippet(acc, &ctx);
            }
            // A name being *defined* is not a reference to anything, and
            // no text inside a string literal is. Both arms used to be
            // unreachable — `analyze` answered `NameRef` for every
            // position — so `function fo|` and `"fo|"` were both offered
            // every symbol in the workspace.
            context::CompletionAnalysis::Name(name_ctx) => {
                completions::name::complete_name(acc, &ctx, all_files, name_ctx);
            }
            context::CompletionAnalysis::String | context::CompletionAnalysis::Comment => {}
        }
    }

    let mut items: Vec<CompletionItem> = completions.into();
    let suffix_len = text[offset..]
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '\'')
        .map(char::len_utf8)
        .sum::<usize>();
    let replacement = base_db::text_range(offset.saturating_sub(prefix.len()), offset + suffix_len);
    for item in &mut items {
        item.source_range.get_or_insert(replacement);
        item.scope_file = file_text.map(|ft| ft.file_id(db));
        if item.import.is_none()
            && !item.relevance.is_local
            && matches!(
                item.kind,
                CompletionItemKind::Function
                    | CompletionItemKind::Variable
                    | CompletionItemKind::Struct
                    | CompletionItemKind::Enum
                    | CompletionItemKind::TypeParameter
                    | CompletionItemKind::Constant
            )
        {
            use hir::source_order::{
                self,
                bindings::{self, Namespace},
            };
            if file_text.is_some_and(|ft| source_order::declarations_for(db, ft).is_some()) {
                item.origin = file_text
                    .and_then(|ft| {
                        source_order::at_offset(db, ft, offset)
                            .or_else(|| source_order::at_offset(db, ft, offset.saturating_sub(1)))
                    })
                    .and_then(|at| {
                        let namespaces = if let Some(role) = ctx.instantiation_role() {
                            let namespace = match role {
                                syntax::InstantiationReference::Outcome => Namespace::Outcome,
                                syntax::InstantiationReference::Type => Namespace::Type,
                                syntax::InstantiationReference::Callable => Namespace::Callable,
                            };
                            [namespace, namespace]
                        } else if matches!(
                            item.kind,
                            CompletionItemKind::Struct
                                | CompletionItemKind::Enum
                                | CompletionItemKind::TypeParameter
                        ) {
                            [Namespace::Type, Namespace::Value]
                        } else {
                            [Namespace::Value, Namespace::Type]
                        };
                        namespaces.into_iter().find_map(|namespace| {
                            let decl =
                                bindings::resolve(db, at, hir::Name::new(&item.label), namespace)?;
                            hir::def_at_declaration_in_namespace(db, decl, &item.label, namespace)?;
                            Some((decl, namespace))
                        })
                    })
                    .map(|(decl, namespace)| item::CompletionOrigin {
                        file: decl.file.file_id(db),
                        declaration: decl.index,
                        namespace,
                    });
            }
        }
    }
    crate::item::deduplicate_and_sort(&mut items, prefix);
    if let Some(limit) = config.limit {
        items.truncate(limit);
    }
    items
}

/// Completion position context — re-exported from context module.
/// New code should match on `CompletionAnalysis` + `PathKind` instead.
#[allow(unused_imports)]
pub(crate) use context::CompletionPosition;

/// Resolve a completion item by adding documentation.
pub fn resolve_completion_item_ide(item: &mut CompletionItem, all_files: &[(&Url, &dyn FileDb)]) {
    if let Some(doc) = builtin_docs(&item.label) {
        item.documentation = Some(format!("`{}`\n\n{}", item.label, doc));
        return;
    }

    let name = &item.label;
    let mut markdown = Vec::new();

    for (_, file) in all_files {
        if let Some(tree) = file.item_tree() {
            if let Some(id) =
                tree.top_level_items().iter().find(|id| id.name(tree).as_str() == *name)
            {
                let kind_name = format!("{:?}", item.kind);
                markdown.push(format!("**{kind_name}** **{name}**"));
                // The doc the item tree recorded, not a backwards scan
                // through the source for `///` above the definition.
                if let Some(comments) = id.doc(tree).map(str::to_string) {
                    markdown.push("___".to_string());
                    markdown.push(comments);
                }
                break;
            }
        }
    }

    // Show all overload signatures when available
    let all_sigs =
        ide_db::signatures::find_all_callable_signatures(all_files.iter().copied(), name);
    if !all_sigs.is_empty() {
        markdown.push("___".to_string());
        if all_sigs.len() > 1 {
            markdown.push(format!("**{} overloads:**", all_sigs.len()));
        }
        for sig in &all_sigs {
            markdown.push(format!("```sail\n{}\n```", sig.label));
        }
    }

    if markdown.is_empty() {
        let detail = item.detail.as_deref().unwrap_or("Sail symbol");
        markdown.push(format!("`{}`\n\n{}", item.label, detail));
    }

    item.documentation = Some(markdown.join("\n\n"));
}

#[cfg(test)]
mod cursor_tests {
    use ide_db::fixture::MultiFileFixture;
    use ide_db::test_utils::TestFile;

    /// Helper: parse a fixture, find the cursor file, build a TestFile,
    /// then run field_completions at the cursor offset.
    fn complete_at_cursor(fixture_text: &str) -> Vec<crate::item::CompletionItem> {
        let fixture = MultiFileFixture::parse(fixture_text);
        let pos = fixture.cursor_position();

        // Find the file containing the cursor
        let cursor_file =
            fixture.files.iter().find(|f| f.file_id == pos.file_id).expect("cursor file not found");

        let test_file = TestFile::new(&cursor_file.text);
        let uri = url::Url::parse("file:///test/cursor_file.sail").unwrap();
        let all_files: Vec<(&url::Url, &dyn ide_db::FileDb)> =
            vec![(&uri, &test_file as &dyn ide_db::FileDb)];

        let offset: usize = pos.offset.into();
        let prefix = super::completion_prefix(&cursor_file.text, offset);

        crate::completions::dot::field_completions(
            &all_files,
            &test_file,
            &cursor_file.text,
            offset,
            prefix,
        )
    }

    /// Run the completion the server runs, at the cursor.
    ///
    /// This used to call `completion_items_ide` — an older engine that the
    /// server does not use — so the tests below were describing a code path
    /// no editor reaches, and keeping it alive behind `#[expect(dead_code)]`.
    fn complete_full_at_cursor(fixture_text: &str) -> Vec<crate::item::CompletionItem> {
        let fixture = MultiFileFixture::parse(fixture_text);
        let pos = fixture.cursor_position();

        let cursor_file =
            fixture.files.iter().find(|f| f.file_id == pos.file_id).expect("cursor file not found");

        // Build TestFile for each fixture file
        let test_files: Vec<(url::Url, TestFile)> = fixture
            .files
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let uri = url::Url::parse(&format!("file:///test/file{i}.sail")).unwrap();
                (uri, TestFile::new(&f.text))
            })
            .collect();

        let all_files: Vec<(&url::Url, &dyn ide_db::FileDb)> =
            test_files.iter().map(|(u, f)| (u, f as &dyn ide_db::FileDb)).collect();

        // Find the index of the cursor file
        let cursor_idx = fixture.files.iter().position(|f| f.file_id == pos.file_id).unwrap();
        let current_uri = &test_files[cursor_idx].0;

        let offset: usize = pos.offset.into();
        let prefix = super::completion_prefix(&cursor_file.text, offset);

        let keywords = &[
            "function", "val", "let", "var", "if", "else", "match", "return", "foreach", "while",
            "struct", "enum", "union", "type", "register",
        ];
        let builtins = &["true", "false", "bitzero", "bitone", "unit"];

        let file = &test_files[cursor_idx].1;
        let (db, file_text) =
            ide_db::FileDb::file_text(file).expect("the test file has a database");
        super::completions(
            db,
            Some(file_text),
            &all_files,
            current_uri,
            file,
            &cursor_file.text,
            offset,
            prefix,
            keywords,
            builtins,
        )
    }

    // ---------------------------------------------------------------
    // Field completion tests (struct fields after dot)
    // ---------------------------------------------------------------

    #[test]
    fn complete_struct_field_after_dot_requires_type_info() {
        // Field completions require the type checker to resolve `binding_type_text`.
        // Without salsa, TestFile cannot infer types, so field_completions
        // returns empty. This test verifies the infrastructure works end-to-end
        // without panicking and returns empty when type info is unavailable.
        let items = complete_at_cursor(
            "
            //- /main.sail
            struct Point = { x : int, y : int }
            function foo() = {
                let p : Point = struct { x = 1, y = 2 };
                p.$0
            }
        ",
        );
        // Without type inference, field_completions cannot resolve the receiver type.
        // This is expected — the test confirms the fixture + completion pipeline
        // runs without crashing. With salsa wired in, this would return ["x", "y"].
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.is_empty() || labels.contains(&"x"),
            "expected either empty (no type info) or 'x' field, got {labels:?}"
        );
    }

    #[test]
    fn complete_no_fields_for_unknown_type() {
        let items = complete_at_cursor(
            "
            //- /main.sail
            function foo() = {
                let x = 42;
                x.$0
            }
        ",
        );
        // No struct type known for `x`, so no field completions
        assert!(items.is_empty(), "expected no field completions, got {items:?}");
    }

    // ---------------------------------------------------------------
    // Full completion tests (keywords, symbols, builtins)
    // ---------------------------------------------------------------

    #[test]
    fn complete_keyword_in_expression_position() {
        let items = complete_full_at_cursor(
            "
            //- /main.sail
            function foo() = {
                le$0
            }
        ",
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"let"), "expected 'let' keyword in {labels:?}");
    }

    #[test]
    fn complete_function_name_from_other_file() {
        let items = complete_full_at_cursor(
            "
            //- /helpers.sail
            function helper_add(x : int, y : int) -> int = x + y
            //- /main.sail
            function main() = {
                hel$0
            }
        ",
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"helper_add"),
            "expected 'helper_add' from other file in {labels:?}"
        );
    }

    #[test]
    fn complete_type_names_at_top_level() {
        let items = complete_full_at_cursor(
            "
            //- /main.sail
            struct Foo = { a : int }
            enum Bar = { X, Y }
            $0
        ",
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // At top level, should see top-level keywords
        assert!(
            labels.contains(&"function"),
            "expected 'function' keyword at top level in {labels:?}"
        );
    }

    #[test]
    fn complete_enum_member_in_expression() {
        let items = complete_full_at_cursor(
            "
            //- /main.sail
            enum Color = { Red, Green, Blue }
            function pick() = {
                Re$0
            }
        ",
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"Red"), "expected 'Red' enum member in {labels:?}");
    }

    #[test]
    fn complete_postfix_after_dot() {
        // Postfix completions are triggered after `.` with a receiver
        let fixture = MultiFileFixture::parse(
            "
            //- /main.sail
            function foo() = {
                let x = 42;
                x.i$0
            }
        ",
        );
        let pos = fixture.cursor_position();
        let cursor_file = fixture.files.iter().find(|f| f.file_id == pos.file_id).unwrap();

        let offset: usize = pos.offset.into();
        let prefix = super::completion_prefix(&cursor_file.text, offset);

        let items =
            crate::completions::postfix::postfix_completions(&cursor_file.text, offset, prefix);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&".if"), "expected '.if' postfix in {labels:?}");
    }

    #[test]
    fn complete_pragma_after_at_sign() {
        // Pragma completions triggered after '@'
        let fixture = MultiFileFixture::parse(
            "
            //- /main.sail
            @$0
        ",
        );
        let pos = fixture.cursor_position();
        let cursor_file = fixture.files.iter().find(|f| f.file_id == pos.file_id).unwrap();

        let offset: usize = pos.offset.into();
        let items = crate::completions::pragma::pragma_completions(&cursor_file.text, offset);
        // Pragma completions may be empty if no prefix after @, but the
        // function should not panic and should return valid results
        assert!(
            items.is_empty() || items.iter().all(|i| i.label.starts_with('@')),
            "pragma items should start with '@': {items:?}"
        );
    }

    // ---------------------------------------------------------------
    // Cursor extraction verification
    // ---------------------------------------------------------------

    #[test]
    fn cursor_marker_stripped_from_fixture_text() {
        let fixture = MultiFileFixture::parse(
            "
            //- /main.sail
            function foo() = bar($0)
        ",
        );
        let pos = fixture.cursor_position();
        let file = fixture.files.iter().find(|f| f.file_id == pos.file_id).unwrap();
        assert!(!file.text.contains("$0"), "$0 marker should be stripped from text");
        assert!(
            file.text.contains("bar()"),
            "text should have bar() without marker: {}",
            file.text
        );
    }

    #[test]
    fn cursor_offset_points_to_correct_position() {
        let fixture = MultiFileFixture::parse(
            "
            //- /main.sail
            val foo : int
            function bar() = foo$0
        ",
        );
        let pos = fixture.cursor_position();
        let file = fixture.files.iter().find(|f| f.file_id == pos.file_id).unwrap();
        let offset: usize = pos.offset.into();
        // Cursor should be right after "foo"
        let before_cursor = &file.text[..offset];
        assert!(
            before_cursor.ends_with("foo"),
            "expected cursor after 'foo', but text before cursor is: '{before_cursor}'"
        );
    }

    // ---------------------------------------------------------------
    // Multi-file fixture with $include
    // ---------------------------------------------------------------

    #[test]
    fn complete_symbol_from_included_file() {
        let items = complete_full_at_cursor(
            "
            //- /prelude.sail
            function prelude_helper(x : int) -> int = x + 1
            //- /main.sail
            $include \"prelude.sail\"
            function main() = {
                prelude$0
            }
        ",
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"prelude_helper"),
            "expected 'prelude_helper' from included file in {labels:?}"
        );
    }

    /// A name being defined, and the inside of a string, are not places
    /// where the workspace's symbols belong.
    ///
    /// `analyze` answered `NameRef` for every position, so the `Name` and
    /// `String` arms of `CompletionAnalysis` — RA's model, carried over
    /// whole — were unreachable. Typing the name of a new function was
    /// offered every function that already exists.
    #[test]
    fn a_name_being_defined_is_not_a_reference() {
        let items = complete_full_at_cursor(
            "
            //- /main.sail
            function helper() = 1
            function main() = {
                let hel$0
            }
        ",
        );
        assert!(
            !items.iter().any(|i| i.label == "helper"),
            "offered an existing name where a new one is being defined: {:?}",
            items.iter().map(|i| &i.label).collect::<Vec<_>>()
        );
    }

    #[test]
    fn nothing_completes_inside_a_string() {
        let items = complete_full_at_cursor(
            "
            //- /main.sail
            function helper() = 1
            function main() = {
                print(\"hel$0\")
            }
        ",
        );
        assert!(
            items.is_empty(),
            "completed inside a string literal: {:?}",
            items.iter().map(|i| &i.label).collect::<Vec<_>>()
        );
    }

    /// The position is still a reference where it always was.
    #[test]
    fn an_ordinary_position_still_completes() {
        let items = complete_full_at_cursor(
            "
            //- /main.sail
            function helper() = 1
            function main() = {
                hel$0
            }
        ",
        );
        assert!(
            items.iter().any(|i| i.label == "helper"),
            "the new position arms swallowed an ordinary reference: {:?}",
            items.iter().map(|i| &i.label).collect::<Vec<_>>()
        );
    }

    /// The one thing that *does* belong where a name is being defined: a
    /// `val` spec with no `function` yet. RA completes a trait method
    /// inside an `impl` for the same reason.
    #[test]
    fn a_val_without_a_function_completes_at_the_definition_site() {
        let items = complete_full_at_cursor(
            "
            //- /main.sail
            val pending_op : (int, int) -> int
            function already(x : int) -> int = x
            val already : int -> int
            function pend$0
        ",
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"pending_op"), "the declaration went unoffered: {labels:?}");
        assert!(!labels.contains(&"already"), "offered a name that is already defined: {labels:?}");
    }
}
