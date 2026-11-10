//! Values of Sail's automatically generated enum-to-number conversion.
//! Sail has no Rust-style `Variant = 42` discriminants. Suppressed or manually
//! supplied conversion functions do not justify a numeric value hint.
use super::*;

pub(super) fn collect_discriminant_hints(
    file: &dyn FileDb,
    begin: usize,
    end: usize,
    hints: &mut Vec<InlayHint>,
    config: &InlayHintsConfig,
) {
    if !config.discriminant_hints {
        return;
    }
    let Some(tree) = file.item_tree() else { return };
    for &owner in
        tree.top_level_items().iter().filter(|item| item.def_kind(tree) == hir::DefKind::Enum)
    {
        let Some(conversion) = owner.enum_number_conversion(tree) else { continue };
        for (ordinal, member) in owner.fields(tree).iter().enumerate() {
            if !span_ends_in_range(member.name_span, begin, end) {
                continue;
            }
            hints.push(InlayHint {
                label_parts: Vec::new(),
                offset: member.name_span.end,
                label: format!("= {ordinal}"),
                kind: InlayHintKind::Other,
                tooltip: Some(format!("{}({}) = {ordinal}", conversion, member.name)),
                padding_left: Some(true),
                padding_right: Some(false),
                data: Some(
                    serde_json::json!({"kind":"enumValue","conversion":conversion.as_str()}),
                ),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hints(source: &str, enabled: bool) -> Vec<InlayHint> {
        let (analysis, files) = crate::fixture::project_analysis(source);
        analysis
            .inlay_hints(
                files[0].0,
                base_db::text_range(0, source.len()),
                &InlayHintsConfig { discriminant_hints: enabled, ..Default::default() },
            )
            .unwrap()
            .into_iter()
            .filter(|hint| hint.data.as_ref().is_some_and(|data| data["kind"] == "enumValue"))
            .collect()
    }

    #[test]
    fn enum_values_follow_sail_generated_conversion_order() {
        let source = "enum Color = { Red, Green, Blue }\n";
        let values = hints(source, true);
        assert_eq!(
            values.iter().map(|hint| hint.label.as_str()).collect::<Vec<_>>(),
            ["= 0", "= 1", "= 2"]
        );
        assert!(values[1].tooltip.as_ref().unwrap().contains("num_of_Color(Green) = 1"));
        assert_eq!(values[1].offset, source.find("Green").unwrap() + 5);
        assert!(hints(source, false).is_empty());
    }

    #[test]
    fn attributes_and_manual_conversions_are_respected() {
        let custom = "$[enum_number_conversions {to_enum = \"decode_color\", from_enum = \"encode_color\"}]\nenum Color = { Red, Green }\n";
        let values = hints(custom, true);
        assert_eq!(values.len(), 2);
        assert!(values[0].tooltip.as_ref().unwrap().contains("encode_color(Red)"));
        for source in [
            "$[no_enum_number_conversions]\nenum Color = { Red, Green }\n",
            "val num_of_Color : Color -> int\nenum Color = { Red, Green }\n",
            "$ifdef NEVER\nenum Color = { Red, Green }\n$endif\n",
            "scattered enum Color\nenum clause Color = Red\nend Color\n",
        ] {
            assert!(hints(source, true).is_empty(), "{source}");
        }
    }
}
