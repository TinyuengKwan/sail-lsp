//! Inlay Hints — parameter names, types, caller counts, closing braces.
//! Architecture: `inlay_hints()` walks the file and collects `InlayHint`
//! values with `InlayKind` tags. Hints can be lazy-resolved via
//! `inlay_hints_resolve()`.

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

/// Kind of inlay hint.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum InlayKind {
    /// Type annotation for a binding.
    Type,
    /// Parameter name hint at call site.
    Parameter,
    /// Closing brace label (e.g., `// fn foo`).
    ClosingBrace,
    /// Caller count hint.
    CallerCount,
    /// Effect annotation hint.
    ///
    /// Shows observed effects at function definition sites
    /// (e.g., `/* throw, exit */`).
    Effect,
    /// Constant value hint.
    ///
    /// Shows the evaluated numeric value of constant expressions inline,
    /// e.g., `sizeof(xlen)` -> `= 64`. Useful for Sail's dependent type
    /// system where `sizeof()` calls are ubiquitous but their concrete
    /// values depend on configuration.
    ConstantValue,
}

// `InlayHintPosition` and a second `InlayHint` — range, position,
// pad_left/pad_right — used to sit here, in rust-analyzer's shape, with no
// constructor: every provider builds the one the server converts, which
// lived in `ide-db` and now lives beside them in `types.rs`.

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
            discriminant_hints: true,
        }
    }
}

fn span_starts_in_range(span: Span, begin: usize, end: usize) -> bool {
    span.start >= begin && span.start <= end
}

/// Inlay hints — returns internal InlayHint (byte-offset positions).
pub fn inlay_hints_ide(
    all_files: &[(&Url, &dyn FileDb)],
    current_uri: &Url,
    current_file: &dyn FileDb,
    range: TextRange,
) -> Vec<InlayHint> {
    inlay_hints_ide_with_types(
        all_files,
        current_uri,
        current_file,
        range,
        None,
        None,
        &InlayHintsConfig::default(),
    )
}

/// Inlay hints with optional salsa-provided TypeCheckResult for type hints.
/// Added `transitive_effects` parameter for salsa-cached effects.
pub fn inlay_hints_ide_with_types(
    all_files: &[(&Url, &dyn FileDb)],
    current_uri: &Url,
    current_file: &dyn FileDb,
    range: TextRange,
    type_check: Option<&hir::TypeCheckResult>,
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
        param_name::collect_hir_parameter_hints(
            all_files,
            current_uri,
            current_file,
            begin,
            end,
            &mut hints,
        );
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
        bind_pat::collect_type_hints(current_file, begin, end, &mut hints, type_check);
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
    chaining::collect_chaining_hints(all_files, current_file, begin, end, &mut hints, config);
    discriminant::collect_discriminant_hints(current_file, begin, end, &mut hints, config);

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

    fn full_range(file: &TestFile) -> TextRange {
        base_db::text_range(0, file.text().len())
    }

    #[test]
    fn emits_caller_count_inlay_hint_for_called_function() {
        let source = "function helper() = 0\nfunction main() = helper()\n";
        let file = TestFile::new(source);
        let uri = Url::parse("file:///tmp/test.sail").unwrap();
        let all_files: Vec<(&Url, &dyn FileDb)> = vec![(&uri, &file as &dyn FileDb)];
        let hints = inlay_hints_ide(&all_files, &uri, &file, full_range(&file));
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
        let hints = inlay_hints_ide(&all_files, &uri, &file, full_range(&file));
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
        let hints = inlay_hints_ide(&all_files, &uri, &file, full_range(&file));
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
        let hints = inlay_hints_ide(&all_files, &uri, &file, full_range(&file));
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
        let hints = inlay_hints_ide(&all_files, &uri, &file, full_range(&file));

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
        let hints = inlay_hints_ide(&all_files, &uri, &file, full_range(&file));

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
        let hints = inlay_hints_ide(&all_files, &uri, &file, full_range(&file));

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
        let hints = inlay_hints_ide(&all_files, &uri, &file, full_range(&file));

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
        let hints = inlay_hints_ide(&all_files, &uri, &file, full_range(&file));

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
        let hints = inlay_hints_ide(&all_files, &uri, &file, full_range(&file));

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
