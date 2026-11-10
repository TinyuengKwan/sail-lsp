//! Inlay Hints — parameter names, types, caller counts, closing braces.
//! Providers collect hints for the requested viewport; Analysis adds navigation links.

mod types;
pub use types::*;
mod bind_pat;
mod call_effect;
mod caller_count;
mod chaining;
mod closing_brace;
mod discriminant;
mod effect;
mod param_name;

use base_db::TextRange;
use ide_db::FileDb;
use parser::Span;
use url::Url;

/// Configuration for inlay hint computation.
#[derive(Clone, Debug)]
pub struct InlayHintsConfig {
    /// Maximum displayed Unicode characters; full labels remain in tooltips.
    pub max_length: Option<usize>,
    /// Show type hints for let/var bindings.
    pub type_hints: bool,
    /// Show parameter name hints at call sites.
    pub parameter_hints: bool,
    /// Show closing brace labels.
    pub closing_brace_hints_min_lines: Option<usize>,
    /// Show caller count hints.
    pub caller_count_hints: bool,
    /// Show effect annotation hints at function definitions.
    ///
    /// Displays observed effects (throw, exit, register read/write, etc.)
    /// next to function names.
    pub effect_hints: bool,
    /// Show chaining hints for field access chains.
    pub chaining_hints: bool,
    /// Show discriminant value hints for enum variants.
    pub discriminant_hints: bool,
}

impl Default for InlayHintsConfig {
    fn default() -> Self {
        Self {
            max_length: Some(25),
            type_hints: true,
            parameter_hints: true,
            closing_brace_hints_min_lines: Some(6),
            caller_count_hints: true,
            effect_hints: true,
            chaining_hints: true,
            discriminant_hints: false,
        }
    }
}

fn span_starts_in_range(span: Span, begin: usize, end: usize) -> bool {
    span.start >= begin && span.start <= end
}

fn span_intersects_range(span: Span, begin: usize, end: usize) -> bool {
    span.end >= begin && span.start <= end
}

fn span_ends_in_range(span: Span, begin: usize, end: usize) -> bool {
    span.end >= begin && span.end <= end
}

fn body_groups(file: &dyn FileDb, begin: usize, end: usize) -> Vec<&hir::CallableBodies> {
    if let Some((db, input)) = file.file_text() {
        hir::Semantics::new(db).bodies_in_range(input, Some((begin, end)))
    } else {
        file.bodies().into_iter().collect()
    }
}

pub(crate) fn visible_callable_heads(
    file: &dyn FileDb,
    begin: usize,
    end: usize,
) -> Vec<(String, Span)> {
    if let Some(tree) = file.item_tree() {
        return tree
            .top_level_items()
            .iter()
            .filter(|id| {
                matches!(
                    id.def_kind(tree),
                    hir::DefKind::Function
                        | hir::DefKind::ValSpec
                        | hir::DefKind::Mapping
                        | hir::DefKind::MappingSpec
                )
            })
            .filter(|id| span_ends_in_range(id.name_span(tree), begin, end))
            .map(|id| (id.name(tree).to_string(), id.name_span(tree)))
            .collect();
    }
    file.parsed()
        .into_iter()
        .flat_map(|parsed| &parsed.callable_heads)
        .filter(|head| span_ends_in_range(head.name_span, begin, end))
        .map(|head| (head.name.clone(), head.name_span))
        .collect()
}

pub(crate) fn needs_effects(file: &dyn FileDb, range: TextRange) -> bool {
    let (begin, end) = (base_db::range_start(range), base_db::range_end(range));
    body_groups(file, begin, end).iter().any(|bodies| {
        bodies.entries().iter().any(|entry| {
            span_ends_in_range(entry.def_span, begin, end)
                || (span_intersects_range(entry.def_span, begin, end)
                    && entry.body.iter_exprs().any(|(id, expr)| {
                        matches!(expr, hir::Expr::Call { .. })
                            && entry
                                .source_map
                                .expr_syntax(id)
                                .is_some_and(|span| span_ends_in_range(span, begin, end))
                    }))
        })
    })
}

/// Collect viewport hints with the caller's settings and cached effects.
pub fn inlay_hints_ide(
    all_files: &[(&Url, &dyn FileDb)],
    current_file: &dyn FileDb,
    range: TextRange,
    transitive_effects: Option<
        &std::collections::HashMap<String, std::collections::BTreeSet<hir::EffectTag>>,
    >,
    config: &InlayHintsConfig,
) -> Vec<InlayHint> {
    let begin = base_db::range_start(range);
    let end = base_db::range_end(range);
    let mut hints = Vec::new();

    // Each collector answers to its own flag. The config was previously
    // built here with `Default::default()`, so the settings the client sent
    // were parsed by the server, converted, and then dropped: turning inlay
    // hints off in the editor did nothing.
    if config.parameter_hints {
        param_name::collect_hir_parameter_hints(current_file, begin, end, &mut hints);
    }
    if config.caller_count_hints {
        caller_count::collect_callgraph_caller_hints(
            all_files,
            current_file,
            begin,
            end,
            &mut hints,
        );
    }
    if let Some(min_lines) = config.closing_brace_hints_min_lines {
        closing_brace::collect_closing_brace_hints(current_file, begin, end, min_lines, &mut hints);
    }
    if config.type_hints {
        bind_pat::collect_type_hints(current_file, begin, end, &mut hints);
    }
    if config.effect_hints {
        // Use salsa-cached transitive effects from transitive_effects
        // instead of rebuilding workspace callgraph + fixed-point on every call.
        effect::collect_effect_hints(current_file, begin, end, &mut hints, transitive_effects);
        call_effect::collect_call_effect_hints(
            all_files,
            current_file,
            begin,
            end,
            &mut hints,
            transitive_effects,
        );
    }
    chaining::collect_chaining_hints(current_file, begin, end, &mut hints, config);
    discriminant::collect_discriminant_hints(current_file, begin, end, &mut hints, config);

    hints.retain(|hint| hint.offset >= begin && hint.offset <= end);
    hints.sort_by(|lhs, rhs| lhs.offset.cmp(&rhs.offset).then(lhs.label.cmp(&rhs.label)));
    if let Some(limit) = config.max_length {
        for hint in &mut hints {
            if hint.label.chars().count() > limit {
                let full = std::mem::take(&mut hint.label);
                hint.label = if limit == 0 {
                    String::new()
                } else {
                    full.chars().take(limit - 1).chain(std::iter::once('…')).collect()
                };
                hint.tooltip = Some(match hint.tooltip.take() {
                    Some(tooltip) => format!("{full}\n\n{tooltip}"),
                    None => full,
                });
            }
        }
        hints.retain(|hint| !hint.label.is_empty());
    }
    hints
}

/// Resolve an inlay hint by adding tooltip text based on the data field.
pub fn resolve_inlay_hint(hint: &mut InlayHint) {
    if hint.tooltip.is_none() {
        if let Some(data) = hint.data.as_ref() {
            let kind = data.get("kind").and_then(|value| value.as_str()).unwrap_or("");
            match kind {
                "parameter" => {
                    if let Some(param) = data.get("param").and_then(|value| value.as_str()) {
                        hint.tooltip = Some(format!("Parameter hint for `{param}`"));
                    }
                }
                "type" => {
                    if let Some(ty) = data.get("type").and_then(|value| value.as_str()) {
                        hint.tooltip = Some(format!("Inferred type: `{ty}`"));
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hir::callgraph::SourceFileInfo;
    use ide_db::test_utils::TestFile;

    #[test]
    fn runtime_integer_hints_do_not_expose_private_witness_names() {
        let source = "function identity(argument : int) -> int = argument + 0\nfunction caller(argument : int) -> int = {\n  let observed = identity(argument);\n  let literal = 3;\n  observed\n}\n";
        let (analysis, files) = crate::fixture::project_analysis(source);
        let start = source.find("let observed").unwrap();
        let file = analysis.files_ref().file_text(files[0].0).unwrap();
        let raw =
            hir::Semantics::new(analysis.db()).type_of_pat(file, start + "let ".len()).unwrap();
        assert_eq!(raw.ctor_name(), Some("atom"), "keep the precise runtime singleton internally");
        assert!(!raw.is_fully_known(), "the witness has no source-level constant value");
        assert_eq!(
            hir::Semantics::new(analysis.db()).binding_type_text(
                file,
                parser::Span::new(start + 4, start + "let observed".len())
            ),
            Some(raw.display_text()),
            "source-generating assists must retain the precise type"
        );
        let hints = analysis
            .inlay_hints(
                files[0].0,
                base_db::text_range(start, source.len()),
                &InlayHintsConfig::default(),
            )
            .unwrap();
        let observed = hints
            .iter()
            .find(|hint| {
                hint.offset == start + "let observed".len() && hint.kind == InlayHintKind::Type
            })
            .unwrap();
        assert_eq!(observed.label, ": int", "{hints:?}");
        assert!(
            hints.iter().any(|hint| hint.label == ": atom(3)"),
            "literal indices must remain precise: {hints:?}"
        );
        assert!(hints.iter().all(|hint| !hint.label.contains("'_sk")), "{hints:?}");

        // A source-written index that resembles the allocator's naming
        // convention must remain visible; classification is semantic.
        let source = "val indexed : forall ('_sk2__value : Int). int('_sk2__value) -> int('_sk2__value)\nfunction indexed(value) = { let preserved = value; preserved }\n";
        let (analysis, files) = crate::fixture::project_analysis(source);
        let start = source.find("let preserved").unwrap();
        let hints = analysis
            .inlay_hints(
                files[0].0,
                base_db::text_range(start, source.len()),
                &InlayHintsConfig::default(),
            )
            .unwrap();
        assert!(hints.iter().any(|hint| hint.kind == InlayHintKind::Type && hint.label.contains("'_sk2__value")), "source binder was hidden: {hints:?}");
    }

    #[test]
    fn viewport_type_hints_infer_only_the_visible_callable() {
        use std::sync::{Arc, Mutex};
        let source = "function offscreen() -> int = { let hidden = 9; hidden }\nfunction visible() -> int = { let shown = 1; shown }\nfunction later() -> int = { let last = 2; last }\n";
        let queries = Arc::new(Mutex::new(Vec::new()));
        let observed = queries.clone();
        let mut db = ide_db::root_database::RootDatabase::with_event_callback(Some(Box::new(
            move |event| {
                if let salsa::EventKind::WillExecute { database_key } = event.kind {
                    // Formatting the query key reads Salsa's ingredient metadata;
                    // it does not execute a query or depend on tracing filters.
                    observed.lock().unwrap().push(format!("{database_key:?}"));
                }
            },
        )));
        let file_id = base_db::FileId::from_raw(0);
        let mut change = ide_db::apply_change::Change::new();
        change.change_file(file_id, source);
        change.set_workspace_files(vec![file_id]);
        db.apply_change(change);
        let uri = Url::parse("file:///tmp/viewport.sail").unwrap();
        let files = db.files().clone();
        let urls =
            crate::analysis::UrlMap::new([(uri.clone(), file_id)].into(), [(file_id, uri)].into());
        let analysis = crate::analysis::Analysis::with_url_map(db, files, urls);
        let config = InlayHintsConfig {
            type_hints: true,
            parameter_hints: false,
            effect_hints: false,
            caller_count_hints: false,
            closing_brace_hints_min_lines: None,
            chaining_hints: false,
            discriminant_hints: false,
            max_length: None,
        };
        let start = source.find("shown =").unwrap();
        let range = base_db::text_range(start, start + 5);
        let viewport = analysis.inlay_hints(file_id, range, &config).unwrap();
        assert_eq!(viewport.len(), 1, "{viewport:?}");
        assert_eq!(viewport[0].offset, start + 5);
        assert!(viewport[0].label.contains("1"), "{viewport:?}");
        let executed = queries.lock().unwrap().clone();
        assert_eq!(
            executed.iter().filter(|key| key.starts_with("callable_bodies_for_name(")).count(),
            1,
            "{executed:?}"
        );
        assert!(
            executed.iter().all(|key| !key.starts_with("callable_bodies(")),
            "offscreen body lowering: {executed:?}"
        );
        assert_eq!(
            executed.iter().filter(|key| key.starts_with("infer(")).count(),
            1,
            "{executed:?}"
        );
        assert!(
            executed.iter().all(|key| !key.starts_with("infer_body(")),
            "whole-file inference: {executed:?}"
        );
        queries.lock().unwrap().clear();
        let defaults = analysis.inlay_hints(file_id, range, &InlayHintsConfig::default()).unwrap();
        assert_eq!(format!("{viewport:?}"), format!("{defaults:?}"));
        let default_queries = queries.lock().unwrap().clone();
        assert!(
            default_queries.iter().all(|key| !key.starts_with("callable_bodies(")
                && !key.starts_with("callable_bodies_for_name(")),
            "offscreen lowering with default hints: {default_queries:?}"
        );
        let full =
            analysis.inlay_hints(file_id, base_db::text_range(0, source.len()), &config).unwrap();
        let subset: Vec<_> = full
            .into_iter()
            .filter(|hint| hint.offset >= start && hint.offset <= start + 5)
            .collect();
        assert_eq!(format!("{viewport:?}"), format!("{subset:?}"));
    }

    #[test]
    fn viewport_hints_are_filtered_by_their_display_position() {
        let source = "function risky() = {\n throw(\"err\")\n}\nfunction main() = risky()\n";
        let file = TestFile::new(source);
        let uri = Url::parse("file:///tmp/view.sail").unwrap();
        let all: Vec<(&Url, &dyn FileDb)> = vec![(&uri, &file)];
        let config = InlayHintsConfig {
            closing_brace_hints_min_lines: Some(1),
            max_length: None,
            ..Default::default()
        };
        let full = inlay_hints_ide(&all, &file, full_range(&file), None, &config);
        assert!(!full.is_empty());
        // One-byte viewports include the boundary cases where a node starts
        // outside the view, but its hint is displayed inside it.
        for start in 0..source.len() {
            let actual =
                inlay_hints_ide(&all, &file, base_db::text_range(start, start + 1), None, &config);
            let expected: Vec<_> = full
                .iter()
                .filter(|hint| hint.offset >= start && hint.offset <= start + 1)
                .collect();
            assert_eq!(format!("{actual:?}"), format!("{expected:?}"), "viewport {start}");
        }
    }

    #[test]
    fn parameter_hints_follow_replaced_signatures_and_display_limits() {
        let source = "val pick : int -> int\nfunction pick(first_parameter) = first_parameter\nfunction before() -> int = pick(1)\nval pick : int -> int\nfunction pick(second_parameter) = second_parameter\nfunction after() -> int = pick(2)\n";
        let fixture = format!("//- /main.sail\n{source}");
        let (analysis, files) = crate::fixture::analysis(&fixture);
        let mut config = InlayHintsConfig { max_length: None, ..Default::default() };
        let whole = base_db::text_range(0, source.len());
        let parameter_hints = |config: &InlayHintsConfig| {
            analysis
                .inlay_hints(files[0].0, whole, config)
                .unwrap()
                .into_iter()
                .filter(|hint| hint.kind == InlayHintKind::Parameter)
                .collect::<Vec<_>>()
        };
        let hints = parameter_hints(&config);
        assert_eq!(
            hints.iter().map(|hint| hint.label.as_str()).collect::<Vec<_>>(),
            ["first_parameter:", "second_parameter:"]
        );
        config.max_length = Some(6);
        let short = parameter_hints(&config);
        assert_eq!(short[0].label, "first…");
        assert_eq!(short[1].label, "secon…");
        assert!(short[1].tooltip.as_ref().unwrap().contains("second_parameter:"));
        config.max_length = Some(0);
        assert!(parameter_hints(&config).is_empty());
    }

    #[test]
    fn closing_brace_threshold_uses_the_configured_line_count() {
        let source = "function long_function() -> int = {\n\n\n  1\n}\n";
        let fixture = format!("//- /main.sail\n{source}");
        let (analysis, files) = crate::fixture::analysis(&fixture);
        for (threshold, expected) in [(Some(2), true), (Some(20), false), (None, false)] {
            let config =
                InlayHintsConfig { closing_brace_hints_min_lines: threshold, ..Default::default() };
            let hints = analysis
                .inlay_hints(files[0].0, base_db::text_range(0, source.len()), &config)
                .unwrap();
            assert_eq!(hints.iter().any(|hint| hint.label.contains("// long_function")), expected);
        }
    }

    #[test]
    fn parameter_hints_do_not_borrow_names_from_an_unresolved_foreign_function() {
        let file = TestFile::new("function main() -> int = helper(1)\n");
        let foreign =
            TestFile::new("function helper(unrelated_name : int) -> int = unrelated_name\n");
        let uri = Url::parse("file:///tmp/main.sail").unwrap();
        let foreign_uri = Url::parse("file:///tmp/unrelated.sail").unwrap();
        let all: Vec<(&Url, &dyn FileDb)> = vec![(&uri, &file), (&foreign_uri, &foreign)];
        let hints =
            inlay_hints_ide(&all, &file, full_range(&file), None, &InlayHintsConfig::default());
        assert!(hints.iter().all(|hint| hint.kind != InlayHintKind::Parameter), "{hints:?}");
    }

    fn full_range(file: &TestFile) -> TextRange {
        base_db::text_range(0, file.text().len())
    }

    #[test]
    fn emits_caller_count_inlay_hint_for_called_function() {
        let source = "function helper() = 0\nfunction main() = helper()\n";
        let file = TestFile::new(source);
        let uri = Url::parse("file:///tmp/test.sail").unwrap();
        let all_files: Vec<(&Url, &dyn FileDb)> = vec![(&uri, &file as &dyn FileDb)];
        let hints = inlay_hints_ide(
            &all_files,
            &file,
            full_range(&file),
            None,
            &InlayHintsConfig::default(),
        );
        let caller_hints: Vec<String> =
            hints.iter().filter(|h| h.label.contains("caller")).map(|h| h.label.clone()).collect();
        assert!(
            caller_hints.iter().any(|l| l == "(1 caller)"),
            "expected (1 caller) hint on `helper`, got {caller_hints:?}"
        );
    }

    #[test]
    fn caller_count_inlay_hint_pluralizes_correctly() {
        let source = "\
function shared() = 0
function a() = shared()
function b() = shared()
function c() = shared()
";
        let file = TestFile::new(source);
        let uri = Url::parse("file:///tmp/test.sail").unwrap();
        let all_files: Vec<(&Url, &dyn FileDb)> = vec![(&uri, &file as &dyn FileDb)];
        let hints = inlay_hints_ide(
            &all_files,
            &file,
            full_range(&file),
            None,
            &InlayHintsConfig::default(),
        );
        let labels: Vec<String> = hints.iter().map(|h| h.label.clone()).collect();
        assert!(
            labels.iter().any(|l| l == "(3 callers)"),
            "expected (3 callers) hint on `shared`, got {labels:?}"
        );
    }

    #[test]
    fn caller_count_inlay_hint_skipped_for_zero_callers() {
        let source = "function unused() = 0\n";
        let file = TestFile::new(source);
        let uri = Url::parse("file:///tmp/test.sail").unwrap();
        let all_files: Vec<(&Url, &dyn FileDb)> = vec![(&uri, &file as &dyn FileDb)];
        let hints = inlay_hints_ide(
            &all_files,
            &file,
            full_range(&file),
            None,
            &InlayHintsConfig::default(),
        );
        let caller_hints: Vec<String> =
            hints.iter().filter(|h| h.label.contains("caller")).map(|h| h.label.clone()).collect();
        assert!(
            caller_hints.is_empty(),
            "zero-caller function should get no inlay hint, got {caller_hints:?}"
        );
    }

    #[test]
    fn caller_count_inlay_hint_skips_synthesised_bitfield_accessors() {
        let source = "bitfield Foo : bits(8) = { lo : 3 .. 0 }\n";
        let file = TestFile::new(source);
        let uri = Url::parse("file:///tmp/test.sail").unwrap();
        let all_files: Vec<(&Url, &dyn FileDb)> = vec![(&uri, &file as &dyn FileDb)];
        let hints = inlay_hints_ide(
            &all_files,
            &file,
            full_range(&file),
            None,
            &InlayHintsConfig::default(),
        );
        let caller_hints: Vec<String> =
            hints.iter().filter(|h| h.label.contains("caller")).map(|h| h.label.clone()).collect();
        assert!(
            caller_hints.is_empty(),
            "synthesised bitfield accessors should not get caller hints, got {caller_hints:?}"
        );
    }

    #[test]
    fn emits_parameter_hints_from_ast_calls() {
        let source = r#"
function add(x, y) = x + y
function main() = add(1, 2)
"#;
        let file = TestFile::new(source);
        let uri = Url::parse("file:///tmp/test.sail").unwrap();
        let all_files: Vec<(&Url, &dyn FileDb)> = vec![(&uri, &file as &dyn FileDb)];
        let hints = inlay_hints_ide(
            &all_files,
            &file,
            full_range(&file),
            None,
            &InlayHintsConfig::default(),
        );

        let labels = hints
            .iter()
            .filter(|hint| hint.kind == InlayHintKind::Parameter)
            .map(|h| h.label.clone())
            .collect::<Vec<_>>();
        assert_eq!(labels, vec!["x:", "y:"]);
    }

    #[test]
    fn emits_type_hints_from_ast_bindings() {
        let source = r#"
function main() = {
  let local = 1;
  local
}
let top = 0
let typed : int = 2
"#;
        let file = TestFile::new(source);
        let uri = Url::parse("file:///tmp/test.sail").unwrap();
        let all_files: Vec<(&Url, &dyn FileDb)> = vec![(&uri, &file as &dyn FileDb)];
        let hints = inlay_hints_ide(
            &all_files,
            &file,
            full_range(&file),
            None,
            &InlayHintsConfig::default(),
        );

        let type_hints = hints
            .iter()
            .filter(|hint| hint.kind == InlayHintKind::Type)
            .map(|h| h.label.clone())
            .collect::<Vec<_>>();
        // `let top = 0` gets an inferred hint; `let typed : int = 2` is
        // already annotated, so it gets none.
        assert_eq!(type_hints, vec![": atom(1)"]);
    }

    #[test]
    fn resolves_type_hint_tooltip() {
        let mut hint = InlayHint {
            label_parts: Vec::new(),
            offset: 0,
            label: ": int".to_string(),
            kind: InlayHintKind::Type,
            tooltip: None,
            padding_left: None,
            padding_right: None,
            data: Some(serde_json::json!({
                "kind": "type",
                "type": "int",
            })),
        };

        resolve_inlay_hint(&mut hint);
        assert_eq!(hint.tooltip.as_deref(), Some("Inferred type: `int`"));
    }

    // ---------------------------------------------------------------
    // Call-site effect inlay hint tests
    // ---------------------------------------------------------------

    #[test]
    fn call_site_effect_hint_for_throwing_callee() {
        let source = r#"
function thrower() = throw("boom")
function main() = thrower()
"#;
        let file = TestFile::new(source);
        let uri = Url::parse("file:///tmp/test.sail").unwrap();
        let all_files: Vec<(&Url, &dyn FileDb)> = vec![(&uri, &file as &dyn FileDb)];
        let hints = inlay_hints_ide(
            &all_files,
            &file,
            full_range(&file),
            None,
            &InlayHintsConfig::default(),
        );

        let call_effect_hints: Vec<&str> = hints
            .iter()
            .filter(|h| h.label.contains("throw") && h.label.starts_with("/*"))
            .map(|h| h.label.as_str())
            .collect();
        assert!(
            !call_effect_hints.is_empty(),
            "expected call-site effect hint for throw, got hints: {:?}",
            hints.iter().map(|h| &h.label).collect::<Vec<_>>()
        );
    }

    #[test]
    fn call_site_no_effect_hint_for_pure_callee() {
        let source = r#"
function pure_fn(x) = x + 1
function main() = pure_fn(42)
"#;
        let file = TestFile::new(source);
        let uri = Url::parse("file:///tmp/test.sail").unwrap();
        let all_files: Vec<(&Url, &dyn FileDb)> = vec![(&uri, &file as &dyn FileDb)];
        let hints = inlay_hints_ide(
            &all_files,
            &file,
            full_range(&file),
            None,
            &InlayHintsConfig::default(),
        );

        let call_effect_hints: Vec<&str> = hints
            .iter()
            .filter(|h| h.label.starts_with("/*") && h.label.ends_with("*/"))
            .filter(|h| {
                // Exclude definition-site effect hints (those appear at function defs)
                h.data.as_ref().and_then(|d| d.get("kind")).and_then(|k| k.as_str())
                    == Some("call_effect")
            })
            .map(|h| h.label.as_str())
            .collect();
        assert!(
            call_effect_hints.is_empty(),
            "pure function call should have no effect hints, got {call_effect_hints:?}"
        );
    }

    #[test]
    fn call_site_effect_hint_shows_multiple_effects() {
        let source = r#"
function dangerous() = {
    throw("oops");
    exit()
}
function main() = dangerous()
"#;
        let file = TestFile::new(source);
        let uri = Url::parse("file:///tmp/test.sail").unwrap();
        let all_files: Vec<(&Url, &dyn FileDb)> = vec![(&uri, &file as &dyn FileDb)];
        let hints = inlay_hints_ide(
            &all_files,
            &file,
            full_range(&file),
            None,
            &InlayHintsConfig::default(),
        );

        let call_effect_hints: Vec<&str> = hints
            .iter()
            .filter(|h| {
                h.data.as_ref().and_then(|d| d.get("kind")).and_then(|k| k.as_str())
                    == Some("call_effect")
            })
            .map(|h| h.label.as_str())
            .collect();
        // Should contain both throw and exit
        let combined = call_effect_hints.join(" ");
        assert!(
            combined.contains("throw") && combined.contains("exit"),
            "expected both throw and exit in call-site hints, got {call_effect_hints:?}"
        );
    }

    #[test]
    fn call_site_effect_hint_tooltip_names_callee() {
        let source = r#"
function risky() = throw("err")
function main() = risky()
"#;
        let file = TestFile::new(source);
        let uri = Url::parse("file:///tmp/test.sail").unwrap();
        let all_files: Vec<(&Url, &dyn FileDb)> = vec![(&uri, &file as &dyn FileDb)];
        let hints = inlay_hints_ide(
            &all_files,
            &file,
            full_range(&file),
            None,
            &InlayHintsConfig::default(),
        );

        let tooltip = hints
            .iter()
            .find(|h| {
                h.data.as_ref().and_then(|d| d.get("kind")).and_then(|k| k.as_str())
                    == Some("call_effect")
            })
            .and_then(|h| h.tooltip.clone());
        assert!(
            tooltip.as_deref().unwrap_or("").contains("risky"),
            "tooltip should mention callee name 'risky', got {tooltip:?}"
        );
    }
}
