//! Hint navigation uses the same declaration identities as ordinary navigation.
use super::*;
use crate::inlay_hints::{InlayHint, InlayHintKind, InlayHintLabelPart};
use salsa::Database as _;

impl Analysis {
    pub(super) fn link_inlay_hints(&self, view: FileId, hints: &mut [InlayHint]) {
        let Some(file) = self.file_text(view) else { return };
        let sema = hir::Semantics::new(&self.db);
        for hint in hints {
            self.db.unwind_if_revision_cancelled();
            match hint.kind {
                InlayHintKind::Type => {
                    let tokens = parser::LexedStr::new(&hint.label);
                    let mut offset = 0;
                    let mut pending = String::new();
                    for index in 0..tokens.len() {
                        let value = tokens.text(index);
                        offset += value.len();
                        let location = (tokens.kind(index) == syntax::SyntaxKind::IDENT
                            && !hint.label.get(offset..).is_some_and(|tail| tail.starts_with('…')))
                        .then(|| sema.resolve_type_path(file, hint.offset, value))
                        .flatten()
                        .and_then(|resolution| match resolution {
                            hir::PathResolution::Def(definition) => {
                                let loc = definition.location();
                                let (tree, item) = loc.item(&self.db)?;
                                let span = item.name_span(tree);
                                Some(crate::FileRange {
                                    file_id: base_db::preprocess::physical_file(
                                        &self.db,
                                        loc.file_id(&self.db),
                                    ),
                                    range: base_db::text_range(span.start, span.end),
                                })
                            }
                            _ => None,
                        });
                        if location.is_some() {
                            if !pending.is_empty() {
                                hint.label_parts.push(InlayHintLabelPart {
                                    value: std::mem::take(&mut pending),
                                    location: None,
                                });
                            }
                            hint.label_parts
                                .push(InlayHintLabelPart { value: value.into(), location });
                        } else {
                            pending.push_str(value);
                        }
                    }
                    if !hint.label_parts.is_empty() && !pending.is_empty() {
                        hint.label_parts
                            .push(InlayHintLabelPart { value: pending, location: None });
                    }
                    if offset != hint.label.len() {
                        hint.label_parts.clear();
                    }
                }
                InlayHintKind::Parameter => {
                    let Some(data) = &hint.data else { continue };
                    let Some(callee) = data.get("callee").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    let Some(name) = data.get("param").and_then(|v| v.as_str()) else { continue };
                    let Some(index) = data
                        .get("index")
                        .and_then(|v| v.as_u64())
                        .and_then(|n| usize::try_from(n).ok())
                    else {
                        continue;
                    };
                    if hint.label != format!("{name}:") {
                        continue;
                    }
                    let Some(groups) = sema.callable_parameters_at(file, hint.offset, callee)
                    else {
                        continue;
                    };
                    let Some(first) = groups.first().and_then(|parameters| parameters.get(index))
                    else {
                        continue;
                    };
                    if first.name != name
                        || groups.iter().any(|parameters| parameters.get(index) != Some(first))
                    {
                        continue;
                    }
                    let Some(mut location) = first.source else { continue };
                    location.file_id =
                        base_db::preprocess::physical_file(&self.db, location.file_id);
                    hint.label_parts = vec![
                        InlayHintLabelPart {
                            value: name.into(),
                            location: Some(crate::FileRange {
                                file_id: location.file_id,
                                range: location.range,
                            }),
                        },
                        InlayHintLabelPart { value: ":".into(), location: None },
                    ];
                }
                InlayHintKind::Other => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::hierarchy::tests::{setup, snapshot};
    use crate::inlay_hints::InlayHintsConfig;

    #[test]
    fn type_and_parameter_hints_link_to_their_cross_file_declarations() {
        let definitions = "struct Widget = { count : int }\nfunction build(seed : int) -> Widget = struct { count = seed }\n";
        let main = "function use() -> int = { let item = build(4); item.count }\n";
        let (host, map, _) =
            setup(&[("defs.sail", definitions), ("main.sail", main)], &[0, 1], vec![]);
        let hints = snapshot(&host, &map)
            .inlay_hints(
                FileId::from_raw(1),
                base_db::text_range(0, main.len()),
                &InlayHintsConfig::default(),
            )
            .unwrap();
        for (label, target) in [(": Widget", "Widget"), ("seed:", "seed")] {
            let hint = hints
                .iter()
                .find(|h| h.label == label)
                .unwrap_or_else(|| panic!("missing {label}: {hints:?}"));
            let locations: Vec<_> = hint.label_parts.iter().filter_map(|p| p.location).collect();
            assert_eq!(locations.len(), 1, "{hint:?}");
            assert_eq!(locations[0].file_id, FileId::from_raw(0));
            assert_eq!(usize::from(locations[0].range.start()), definitions.find(target).unwrap());
            assert_eq!(
                hint.label_parts.iter().map(|p| p.value.as_str()).collect::<String>(),
                hint.label
            );
        }
    }

    #[test]
    fn truncated_names_do_not_link_to_a_different_type_with_the_same_prefix() {
        let source = "struct Long = { count : int }\nstruct LongStructName = { count : int }\nfunction build() -> LongStructName = struct { count = 0 }\nfunction use() -> int = { let item = build(); item.count }\n";
        let (host, map, _) = setup(&[("main.sail", source)], &[0], vec![]);
        let config = InlayHintsConfig { max_length: Some(7), ..Default::default() };
        let hints = snapshot(&host, &map)
            .inlay_hints(FileId::from_raw(0), base_db::text_range(0, source.len()), &config)
            .unwrap();
        let hint =
            hints.iter().find(|h| h.label == ": Long…").unwrap_or_else(|| panic!("{hints:?}"));
        assert!(hint.label_parts.is_empty(), "{hint:?}");
        assert!(hint.tooltip.as_ref().unwrap().contains("LongStructName"));
    }

    #[test]
    fn agreeing_parameter_names_do_not_guess_between_different_overload_declarations() {
        let source = "function one(value : int) -> int = value\nfunction two(value : bool) -> bool = value\noverload pick = {one, two}\nfunction use() -> int = pick(1)\n";
        let (host, map, _) = setup(&[("main.sail", source)], &[0], vec![]);
        let hints = snapshot(&host, &map)
            .inlay_hints(
                FileId::from_raw(0),
                base_db::text_range(0, source.len()),
                &InlayHintsConfig::default(),
            )
            .unwrap();
        let hint =
            hints.iter().find(|h| h.label == "value:").unwrap_or_else(|| panic!("{hints:?}"));
        assert!(hint.label_parts.is_empty(), "ambiguous declarations: {hint:?}");
    }

    #[test]
    fn identical_type_labels_keep_no_arbitrary_link_across_include_contexts() {
        use base_db::module_graph::ModuleData;
        let first = "private struct T = { a : int }\n$include \"shared.sail\"\n";
        let second = "private struct T = { b : bool }\n$include \"shared.sail\"\n";
        let shared = "function keep(value : T) -> T = { let copy = value; copy }\n";
        let modules = (0..2)
            .map(|id| ModuleData { files: vec![FileId::from_raw(id)], requires: vec![] })
            .collect();
        let (host, map, _) = setup(
            &[("a.sail", first), ("b.sail", second), ("shared.sail", shared)],
            &[0, 1],
            modules,
        );
        let hints = snapshot(&host, &map)
            .inlay_hints(
                FileId::from_raw(2),
                base_db::text_range(0, shared.len()),
                &InlayHintsConfig::default(),
            )
            .unwrap();
        let hint = hints.iter().find(|h| h.label == ": T").unwrap_or_else(|| panic!("{hints:?}"));
        assert!(hint.label_parts.iter().all(|p| p.location.is_none()), "{hint:?}");
    }
}
