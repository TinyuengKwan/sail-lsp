//! Types of intermediate receivers in a field-access chain.
use super::*;

pub(super) fn collect_chaining_hints(
    file: &dyn FileDb,
    begin: usize,
    end: usize,
    hints: &mut Vec<InlayHint>,
    config: &InlayHintsConfig,
) {
    if !config.chaining_hints {
        return;
    }
    let Some((db, input)) = file.file_text() else { return };
    let Some(root) = syntax::parse_query::semantic_parse_file(db, input).syntax_node() else {
        return;
    };
    let sema = hir::Semantics::new(db);
    for receiver in
        root.descendants().filter(|node| node.kind() == syntax::SyntaxKind::FIELD_ACCESS_EXPR)
    {
        let range = receiver.text_range();
        let offset = usize::from(range.end());
        if offset < begin || offset > end {
            continue;
        }
        if !receiver.parent().is_some_and(|parent| {
            parent.kind() == syntax::SyntaxKind::FIELD_ACCESS_EXPR
                && parent.first_child().as_ref() == Some(&receiver)
        }) {
            continue;
        }
        let span = Span::new(usize::from(range.start()), offset);
        let Some(ty) = sema.expr_type_display(input, span) else { continue };
        hints.push(InlayHint {
            label_parts: Vec::new(),
            offset,
            label: format!(": {ty}"),
            kind: InlayHintKind::Type,
            tooltip: Some("Type of this intermediate field-access result".into()),
            padding_left: Some(false),
            padding_right: Some(true),
            data: Some(serde_json::json!({"kind":"chaining"})),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chained_fields_use_inferred_receiver_types_and_respect_viewport_and_setting() {
        let source = "struct Address = { zip : int }\nstruct Person = { address : Address }\nfunction f(p : Person) -> int = p.address.zip + p.address.zip\n";
        let (analysis, files) = crate::fixture::project_analysis(source);
        let offset = source.find("p.address").unwrap() + "p.address".len();
        for enabled in [false, true] {
            let config = InlayHintsConfig { chaining_hints: enabled, ..Default::default() };
            let hints = analysis
                .inlay_hints(files[0].0, base_db::text_range(offset, offset), &config)
                .unwrap();
            let hints: Vec<_> = hints
                .iter()
                .filter(|hint| hint.data.as_ref().is_some_and(|data| data["kind"] == "chaining"))
                .collect();
            assert_eq!(hints.len(), usize::from(enabled), "{hints:?}");
            if enabled {
                assert_eq!(hints[0].offset, offset);
                assert_eq!(hints[0].label, ": Address");
            }
        }
    }

    #[test]
    fn unresolved_intermediate_fields_do_not_invent_a_type() {
        let source = "function f(x) = x.missing.other\n";
        let (analysis, files) = crate::fixture::project_analysis(source);
        let hints = analysis
            .inlay_hints(files[0].0, base_db::text_range(0, source.len()), &Default::default())
            .unwrap();
        assert!(hints
            .iter()
            .all(|hint| hint.data.as_ref().is_none_or(|data| data["kind"] != "chaining")));
    }
}
