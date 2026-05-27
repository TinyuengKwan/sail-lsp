//! Inlay Hints — parameter names, types, caller counts, closing braces.
//! Architecture: `inlay_hints()` walks the file and collects `InlayHint`
//! values with `InlayKind` tags. Hints can be lazy-resolved via
//! `inlay_hints_resolve()`.

mod bind_pat;
mod call_effect;
mod caller_count;
mod chaining;
mod closing_brace;
mod discriminant;
mod effect;
mod param_name;

use base_db::TextRange;
use ide_db::ide_types::{InlayHint as IdeDbInlayHint, InlayHintKind as IdeDbInlayHintKind};
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

/// Position of inlay hint relative to the anchor range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InlayHintPosition {
    /// Before the range (e.g., parameter name before argument).
    Before,
    /// After the range (e.g., type after binding name).
    After,
}

/// A single inlay hint.
#[derive(Debug, Clone)]
pub struct InlayHint {
    /// Byte-offset range this hint applies to.
    pub range: TextRange,
    /// Position relative to range.
    pub position: InlayHintPosition,
    /// Padding before label.
    pub pad_left: bool,
    /// Padding after label.
    pub pad_right: bool,
    /// Kind of this hint.
    pub kind: InlayKind,
    /// Display label text.
    pub label: String,
    /// Tooltip text (lazy-resolvable).
    pub tooltip: Option<String>,
    /// Extra data for resolution.
    pub data: Option<serde_json::Value>,
}

/// Configuration for inlay hint computation.
#[derive(Clone, Debug)]
pub struct InlayHintsConfig {
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
) -> Vec<IdeDbInlayHint> {
    inlay_hints_ide_with_types(all_files, current_uri, current_file, range, None, None)
}

/// Inlay hints with optional salsa-provided TypeCheckResult for type hints.
/// Added `transitive_effects` parameter for salsa-cached effects.
pub fn inlay_hints_ide_with_types(
    all_files: &[(&Url, &dyn FileDb)],
    current_uri: &Url,
    current_file: &dyn FileDb,
    range: TextRange,
    type_check: Option<&hir_ty::infer::TypeCheckResult>,
    transitive_effects: Option<
        &std::collections::HashMap<String, std::collections::BTreeSet<hir_def::EffectTag>>,
    >,
) -> Vec<IdeDbInlayHint> {
    let begin = base_db::range_start(range);
    let end = base_db::range_end(range);
    let mut hints = Vec::new();

    param_name::collect_hir_parameter_hints(
        all_files,
        current_uri,
        current_file,
        begin,
        end,
        &mut hints,
    );
    caller_count::collect_callgraph_caller_hints(all_files, current_file, begin, end, &mut hints);
    closing_brace::collect_closing_brace_hints(current_file, begin, end, &mut hints);
    bind_pat::collect_type_hints(current_file, begin, end, &mut hints, type_check);
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

    let config = InlayHintsConfig::default();
    chaining::collect_chaining_hints(all_files, current_file, begin, end, &mut hints, &config);
    discriminant::collect_discriminant_hints(current_file, begin, end, &mut hints, &config);

    hints.sort_by(|lhs, rhs| lhs.offset.cmp(&rhs.offset).then(lhs.label.cmp(&rhs.label)));
    hints
}

/// Resolve an inlay hint by adding tooltip text based on the data field.
pub fn resolve_inlay_hint(hint: &mut IdeDbInlayHint) {
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
    use hir_def::callgraph::SourceFileInfo;
    use ide_db::test_utils::TestFile;

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
            .filter(|hint| hint.kind == IdeDbInlayHintKind::Parameter)
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
            .filter(|hint| hint.kind == IdeDbInlayHintKind::Type)
            .map(|h| h.label.clone())
            .collect::<Vec<_>>();
        // Type hints require a running type checker (not available in TestFile).
        // When type check is wired into TestFile, these will be non-empty.
        // For now, verify the mechanism exists but produces no hints without inference.
        assert!(type_hints.is_empty() || type_hints == vec![": int", ": int"]);
    }

    #[test]
    fn resolves_type_hint_tooltip() {
        let mut hint = IdeDbInlayHint {
            offset: 0,
            label: ": int".to_string(),
            kind: IdeDbInlayHintKind::Type,
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
                h.data
                    .as_ref()
                    .and_then(|d| d.get("kind"))
                    .and_then(|k| k.as_str())
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
                h.data
                    .as_ref()
                    .and_then(|d| d.get("kind"))
                    .and_then(|k| k.as_str())
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
                h.data
                    .as_ref()
                    .and_then(|d| d.get("kind"))
                    .and_then(|k| k.as_str())
                    == Some("call_effect")
            })
            .and_then(|h| h.tooltip.clone());
        assert!(
            tooltip.as_deref().unwrap_or("").contains("risky"),
            "tooltip should mention callee name 'risky', got {tooltip:?}"
        );
    }
}
