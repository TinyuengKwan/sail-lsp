//! Effect hints at function **call sites**.
//!
//! Sail-specific: when a function call invokes a callee that has known
//! effects (throw, exit, register read/write, etc.), display a small
//! inlay hint after the call showing those effects.
//!
//! This complements `effect.rs` (definition-site hints) by making
//! effects visible at the *usage* site where they matter for reasoning
//! about control flow and side effects.
//!
//! Example:
//! ```text
//! execute(instr)  /* throw, wreg */
//! ```

use super::*;

/// Collect effect hints at call sites within the visible range.
///
/// For each `Expr::Call` whose callee has non-empty effects in the
/// `transitive_effects` map, emit an inlay hint after the call
/// expression showing the callee's effects.
///
/// Falls back to direct effects from the callee's body when
/// transitive effects are unavailable.
pub(super) fn collect_call_effect_hints(
    all_files: &[(&Url, &dyn FileDb)],
    current_file: &dyn FileDb,
    begin: usize,
    end: usize,
    hints: &mut Vec<IdeDbInlayHint>,
    transitive_effects: Option<
        &std::collections::HashMap<String, std::collections::BTreeSet<hir_def::EffectTag>>,
    >,
) {
    use hir_def::hir::Expr;

    let Some(bodies) = current_file.bodies() else {
        return;
    };

    // Build a fallback effects map from all workspace bodies if
    // transitive_effects is not available.
    let fallback_effects: std::collections::HashMap<
        String,
        std::collections::BTreeSet<hir_def::EffectTag>,
    >;
    let effects_map: &std::collections::HashMap<
        String,
        std::collections::BTreeSet<hir_def::EffectTag>,
    > = if let Some(te) = transitive_effects {
        te
    } else {
        // Collect direct effects from all files' callable bodies.
        let mut map = std::collections::HashMap::new();
        for (_, f) in all_files {
            if let Some(file_bodies) = f.bodies() {
                for entry in file_bodies.entries() {
                    if !entry.effects.is_empty() {
                        map.entry(entry.name.clone())
                            .or_insert_with(std::collections::BTreeSet::new)
                            .extend(entry.effects.iter().copied());
                    }
                }
            }
        }
        fallback_effects = map;
        &fallback_effects
    };

    for entry in bodies.entries() {
        for (id, hir) in entry.body.iter_exprs() {
            let call_span = entry.source_map.expr_syntax(id).unwrap_or(parser::Span::new(0, 0));
            if !span_starts_in_range(call_span, begin, end) {
                continue;
            }

            if let Expr::Call { callee, args: _ } = hir {
                // Resolve callee name
                let callee_name = match entry.body.expr(*callee) {
                    Some(Expr::Ident(name)) => name.as_str(),
                    _ => continue,
                };

                // Skip synthetic accessors (bitfield getters/setters)
                if callee_name.starts_with("_mod_")
                    || callee_name.starts_with("_get_")
                    || callee_name.starts_with("_set_")
                    || callee_name.starts_with("_update_")
                {
                    continue;
                }

                let Some(effects) = effects_map.get(callee_name) else {
                    continue;
                };

                // Filter out non-interesting effect tags
                let effect_strs: Vec<&str> = effects
                    .iter()
                    .filter(|e| {
                        !matches!(
                            e,
                            hir_def::EffectTag::Scattered
                                | hir_def::EffectTag::NonExec
                                | hir_def::EffectTag::Return
                        )
                    })
                    .map(|e| e.as_str())
                    .collect();

                if effect_strs.is_empty() {
                    continue;
                }

                let label = format!("/* {} */", effect_strs.join(", "));
                hints.push(IdeDbInlayHint {
                    offset: call_span.end,
                    label,
                    kind: IdeDbInlayHintKind::Other,
                    tooltip: Some(format!(
                        "Effects of `{callee_name}`: {}",
                        effect_strs.join(", ")
                    )),
                    padding_left: Some(true),
                    padding_right: Some(false),
                    data: Some(serde_json::json!({
                        "kind": "call_effect",
                        "callee": callee_name,
                    })),
                });
            }
        }
    }
}
