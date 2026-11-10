//! The inferred value of an implicit argument at a call site.
//!
//! `val widen : forall 'n 'm, 'm >= 'n. (implicit('m), bits('n)) -> bits('m)`
//! is called as `widen(x)`: the first argument is not written, the checker
//! infers it from the expected type (`implicit('n)`, `type_check.ml`). The
//! hint shows the elided argument the way Sail lets you write it
//! explicitly — `widen(8, x)` — in the place of the parameter-name hints,
//! which is also where rust-analyzer's generic-parameter hints sit.

use super::*;

pub(super) fn collect_implicit_argument_hints(
    current_file: &dyn FileDb,
    begin: usize,
    end: usize,
    hints: &mut Vec<InlayHint>,
) {
    let Some((db, file)) = current_file.file_text() else { return };
    let sema = hir::Semantics::new(db);
    let text = current_file.text();
    for bodies in body_groups(current_file, begin, end) {
        for entry in bodies.entries() {
            if !span_intersects_range(entry.def_span, begin, end) {
                continue;
            }
            for (id, expr) in entry.body.iter_exprs() {
                let hir::Expr::Call { callee, args } = expr else { continue };
                let Some(call_span) = entry.source_map.expr_syntax(id) else { continue };
                if call_span.end < begin || call_span.start > end {
                    continue;
                }
                let Some(callee_span) = entry.source_map.expr_syntax(*callee) else { continue };
                // The confirmed instantiation of this call: `implicit_params`
                // marks the positions the caller did not write, and `ty` is
                // the signature with the inferred values in them.
                let Some(call) = sema.call_instantiation(file, callee_span.start) else { continue };
                if !call.implicit_params.contains(&true) {
                    continue;
                }
                let hir::TyKind::FnPtr(instantiated) = call.ty.kind() else { continue };
                let positions: Vec<usize> = call
                    .implicit_params
                    .iter()
                    .enumerate()
                    .filter_map(|(i, &implicit)| implicit.then_some(i))
                    .collect();
                let values: Vec<String> = positions
                    .iter()
                    .filter_map(|&i| implicit_value(instantiated.params.get(i)?))
                    .collect();
                // An implicit the checker left undecided has no value to show.
                if values.len() != positions.len() {
                    continue;
                }
                // The written arguments follow the implicit ones; the hint
                // goes before the first of them, or after `(` when there is
                // none.
                let Some(offset) = args
                    .first()
                    .and_then(|arg| entry.source_map.expr_syntax(*arg))
                    .map(|span| span.start)
                    .or_else(|| {
                        let call_text = text.get(call_span.start..call_span.end)?;
                        Some(call_span.start + call_text.find('(')? + 1)
                    })
                else {
                    continue;
                };
                // The quantified name the value stands for: `'m` when the
                // substitutions name exactly one variable with this value.
                let names: Vec<String> = values
                    .iter()
                    .map(|value| {
                        let mut named = call
                            .type_arguments
                            .iter()
                            .filter(|(_, ty)| ty.display_text() == *value)
                            .map(|(name, _)| name);
                        match (named.next(), named.next()) {
                            (Some(name), None) => format!(" `{name}`"),
                            _ => String::new(),
                        }
                    })
                    .collect();
                let described: Vec<String> = names
                    .iter()
                    .zip(&values)
                    .map(|(name, value)| format!("implicit argument{name} = {value}"))
                    .collect();
                hints.push(InlayHint {
                    label_parts: Vec::new(),
                    offset,
                    label: format!("{},", values.join(", ")),
                    kind: InlayHintKind::Parameter,
                    tooltip: Some(format!(
                        "{}\n\n`{} : {}`",
                        described.join("\n"),
                        call.target,
                        call.ty.display_text()
                    )),
                    padding_left: None,
                    padding_right: Some(true),
                    data: None,
                });
            }
        }
    }
}

/// `implicit(8)` is `8`; `implicit('m)` is `'m`. Anything else is not an
/// implicit parameter.
fn implicit_value(ty: &hir::Ty) -> Option<String> {
    let text = ty.display_text();
    let inner = text.strip_prefix("implicit(")?.strip_suffix(')')?;
    (!inner.is_empty()).then(|| inner.to_owned())
}

#[cfg(test)]
mod tests {
    use crate::inlay_hints::{InlayHintKind, InlayHintsConfig};

    const WIDEN: &str = "default Order dec\nval widen : forall 'n 'm, 'm >= 'n. (implicit('m), bits('n)) -> bits('m)\nfunction demo(x : bits(4)) -> bits(8) = widen(x)\n";

    fn implicit_hints(
        source: &str,
        config: &InlayHintsConfig,
    ) -> Vec<crate::inlay_hints::InlayHint> {
        let (analysis, files) = crate::fixture::analysis(source);
        analysis
            .inlay_hints(files[0].0, base_db::text_range(0, source.len()), config)
            .expect("not cancelled")
            .into_iter()
            .filter(|hint| {
                hint.tooltip.as_deref().is_some_and(|t| t.starts_with("implicit argument"))
            })
            .collect()
    }

    /// `widen(x)` shows the `8` the checker inferred for `implicit('m)`,
    /// before the first written argument.
    #[test]
    fn the_inferred_implicit_value_is_shown_before_the_first_argument() {
        let hints = implicit_hints(WIDEN, &InlayHintsConfig::default());
        let [hint] = hints.as_slice() else { panic!("one implicit hint, got {hints:?}") };
        assert_eq!(hint.label, "8,");
        assert_eq!(hint.kind, InlayHintKind::Parameter);
        assert_eq!(hint.offset, WIDEN.rfind("x)").unwrap(), "{hint:?}");
        assert_eq!(hint.padding_right, Some(true));
        assert_eq!(
            hint.tooltip.as_deref(),
            Some("implicit argument `'m` = 8\n\n`widen : (implicit(8), bits(4)) -> bits(8)`")
        );
    }

    #[test]
    fn a_call_without_implicit_parameters_has_no_hint() {
        let source = "val f : int -> int\nfunction demo(y : int) -> int = f(y)\n";
        assert!(implicit_hints(source, &InlayHintsConfig::default()).is_empty());
    }

    #[test]
    fn the_flag_turns_the_hint_off() {
        let config = InlayHintsConfig { implicit_argument_hints: false, ..Default::default() };
        assert!(implicit_hints(WIDEN, &config).is_empty());
        assert!(!implicit_hints(WIDEN, &InlayHintsConfig::default()).is_empty());
    }
}
