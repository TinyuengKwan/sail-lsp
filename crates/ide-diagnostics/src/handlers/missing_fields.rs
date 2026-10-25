//! Handler for `AnyDiagnostic::MissingFields`.
//! Fix: insert missing fields with type-aware default values .

use crate::Diagnostic;
use hir::diagnostics::DiagnosticCode;
use ide_db::source_change::SourceChange;
use ide_db::text_edit::TextEdit;

use crate::{DiagnosticsContext, MissingFields};

pub(crate) fn missing_fields(ctx: &DiagnosticsContext<'_>, d: &MissingFields) -> Diagnostic {
    let fields_str = d.missing.join(", ");

    Diagnostic::new(
        DiagnosticCode::SailError("missing-fields"),
        format!("missing fields in `{}`: {fields_str}", d.record_name),
        crate::node_file_range(&d.node),
    )
    .with_fixes(fixes(ctx, d))
}

/// Smart-fill missing fields with type-aware defaults.
///
/// RA fills with `Default::default()`, `new()`, `None`, `0`, `false`.
/// Sail equivalent: int→0, nat→0, bool→false, bit→bitzero,
/// concrete bits→zero literal, string→"", unit→().
fn fixes(ctx: &DiagnosticsContext<'_>, d: &MissingFields) -> Option<Vec<ide_db::assists::Assist>> {
    use syntax::SyntaxKind as SK;
    let file = ctx.file.filter(|ft| ft.file_id(ctx.db()) == d.node.file_id)?;
    let root = ctx.sema.parse(file)?;
    let node = d.node.value.try_to_node(&root)?;
    let mut tokens = node
        .descendants_with_tokens()
        .filter_map(|it| it.into_token())
        .filter(|token| !token.kind().is_trivia())
        .collect::<Vec<_>>();
    let close = tokens.pop()?;
    if close.kind() != SK::R_CURLY {
        return None;
    }
    let separator = match tokens.last()?.kind() {
        SK::L_CURLY => "",
        SK::COMMA => " ",
        _ => ", ",
    };
    let pattern = node.kind() == SK::STRUCT_PAT;
    let fields_str = d.missing.join(", ");

    let missing_text = d
        .missing
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let default = if pattern {
                "_".to_owned()
            } else {
                d.field_types
                    .as_ref()
                    .and_then(|types| types.get(i))
                    .map(|ty| default_for_type(ty))
                    .unwrap_or_else(|| "undefined".to_owned())
            };
            format!("{f} = {default}")
        })
        .collect::<Vec<_>>()
        .join(", ");

    let insert_pos = usize::from(close.text_range().start());

    let insert_range = base_db::text_range(insert_pos, insert_pos);
    let edit = TextEdit { range: insert_range, new_text: format!("{separator}{missing_text}") };
    let source_change = SourceChange::from_text_edit(edit);
    Some(vec![crate::fix(
        "fill_missing_fields",
        &format!("Fill missing field(s): {fields_str}"),
        source_change,
        insert_range,
    )])
}

use ide_db::assists::default_value_for_type as default_for_type;

#[cfg(test)]
mod tests {
    use crate::tests::{check_dispatch, test_node};
    use hir::diagnostics::*;

    #[test]
    fn smoke() {
        let diag = AnyDiagnostic::MissingFields(MissingFields {
            record_name: "MyRecord".to_string(),
            missing: vec!["field_a".to_string(), "field_b".to_string()],
            field_types: None,
            node: test_node(0, 10),
        });
        let d = check_dispatch(&diag);
        assert!(d.is_some());
        assert!(d.unwrap().message.contains("missing fields in `MyRecord`"));
    }

    #[test]
    fn default_for_type_values() {
        assert_eq!(super::default_for_type("int"), "0");
        assert_eq!(super::default_for_type("bool"), "false");
        assert_eq!(super::default_for_type("bit"), "bitzero");
        assert_eq!(super::default_for_type("string"), "\"\"");
        assert_eq!(super::default_for_type("bits(8)"), "0x00");
        assert_eq!(super::default_for_type("unknown_type"), "undefined");
    }
}

#[cfg(test)]
mod integration_tests {
    fn apply_fix(source: &str) -> String {
        let file = ide_db::test_utils::TestFile::new(source);
        let diagnostics = crate::semantic_diagnostics(
            file.db(),
            &crate::DiagnosticsConfig::new(),
            &ide_db::assists::AssistResolveStrategy::All,
            &file,
            file.input(),
            None,
        );
        let diagnostic =
            diagnostics.iter().find(|d| d.code.as_str() == "missing-fields").unwrap_or_else(|| {
                panic!(
                    "no typed missing-fields diagnostic: {:?}",
                    diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>()
                )
            });
        let fix = diagnostic
            .fixes
            .as_ref()
            .unwrap()
            .iter()
            .find(|fix| fix.id.0 == "fill_missing_fields")
            .unwrap();
        let mut result = source.to_owned();
        for edit in fix.source_change.as_ref().unwrap().source_file_edits.values().flatten() {
            result.replace_range(
                usize::from(edit.range.start())..usize::from(edit.range.end()),
                &edit.new_text,
            );
        }
        assert!(syntax::parse_text(&result).1.is_empty(), "{result}");
        let fixed = ide_db::test_utils::TestFile::new(&result);
        let remaining = crate::semantic_diagnostics(
            fixed.db(),
            &crate::DiagnosticsConfig::new(),
            &ide_db::assists::AssistResolveStrategy::All,
            &fixed,
            fixed.input(),
            None,
        );
        assert!(
            !remaining
                .iter()
                .any(|d| d.code.as_str() == "missing-fields" || d.code.as_str() == "type-error"),
            "{result}\n{:?}",
            remaining.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        result
    }

    #[test]
    fn fills_record_fields_with_their_types_including_empty_literals() {
        for literal in ["struct S {}", "struct S { n = 1 }", "struct S { n = 1, }"] {
            let result = apply_fix(&format!("struct S = {{ n : int, yes : bool, label : string, data : bits(8) }}\nfunction f() -> S = {literal}\n"));
            assert!(result.contains("yes = false"), "{result}");
            assert!(result.contains("label = \"\""), "{result}");
            assert!(result.contains("data = 0x00"), "{result}");
        }
    }

    #[test]
    fn instantiates_generic_field_types() {
        let result = apply_fix("struct S('n : Int) = { data : bits('n), yes : bool }\nfunction f() -> S(8) = struct S { yes = true }\n");
        assert!(result.contains("data = 0x00"), "{result}");
        let result = apply_fix("struct S('n : Int) = { data : bits('n), yes : bool }\nfunction f(s : S(8)) -> bits(8) = match s { struct S { data } => data }\n");
        assert!(result.contains("yes = _"), "{result}");
    }

    #[test]
    fn fills_patterns_with_wildcards_not_constants() {
        let result = apply_fix("struct S = { x : int, y : bool }\nfunction f(s : S) -> int = match s { struct S { x } => x }\n");
        assert!(result.contains("y = _"), "{result}");
    }
}
