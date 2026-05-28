//! Handler for `AnyDiagnostic::MissingFields`.
//! Fix: insert missing fields with type-aware default values .

use crate::Diagnostic;
use hir_def::diagnostics::DiagnosticCode;
use ide_db::source_change::SourceChange;
use ide_db::text_edit::TextEdit;

use crate::{DiagnosticsContext, MissingFields};

pub(crate) fn missing_fields(_ctx: &DiagnosticsContext<'_>, d: &MissingFields) -> Diagnostic {
    let fields_str = d.missing.join(", ");

    Diagnostic::new(
        DiagnosticCode::SailError("missing-fields"),
        format!("missing fields in `{}`: {fields_str}", d.record_name),
        crate::node_file_range(&d.node),
    )
    .with_fixes(fixes(d))
}

/// Smart-fill missing fields with type-aware defaults.
///
/// RA fills with `Default::default()`, `new()`, `None`, `0`, `false`.
/// Sail equivalent: int→0, nat→0, bool→false, bit→bitzero,
/// bits→sail_zeros(), string→"", unit→().
fn fixes(d: &MissingFields) -> Option<Vec<ide_db::assists::Assist>> {
    let range: ide_db::line_index::TextRange = d.node.value.text_range();
    let fields_str = d.missing.join(", ");

    let missing_text = d
        .missing
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let default = d
                .field_types
                .as_ref()
                .and_then(|types| types.get(i))
                .map(|ty| default_for_type(ty))
                .unwrap_or("()");
            format!("{f} = {default}")
        })
        .collect::<Vec<_>>()
        .join(", ");

    let insert_pos = if base_db::range_end(range) > base_db::range_start(range) {
        base_db::range_end(range) - 1
    } else {
        base_db::range_end(range)
    };

    let insert_range = base_db::text_range(insert_pos, insert_pos);
    let edit = TextEdit { range: insert_range, new_text: format!(", {missing_text}") };
    let source_change = SourceChange::from_text_edit(edit);
    Some(vec![crate::fix(
        "fill_missing_fields",
        &format!("Fill missing field(s): {fields_str}"),
        source_change,
        insert_range,
    )])
}

/// Return a sensible default value for a Sail type name.
///
/// `false`, `None` etc. based on the type.
fn default_for_type(ty: &str) -> &str {
    match ty {
        "int" | "nat" => "0",
        "bool" => "false",
        "bit" => "bitzero",
        "unit" => "()",
        "string" => "\"\"",
        "real" => "0.0",
        _ if ty.starts_with("bits") || ty.starts_with("bitvector") => "sail_zeros()",
        _ => "()",
    }
}

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
        assert_eq!(super::default_for_type("bits(8)"), "sail_zeros()");
        assert_eq!(super::default_for_type("unknown_type"), "()");
    }
}
