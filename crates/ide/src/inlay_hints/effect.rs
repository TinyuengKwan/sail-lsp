//! Effect annotation hints at function definition sites.
//!
//! Sail-specific: shows observed effects (throw, exit, register access, etc.)
//! next to function names. Sail has an explicit effect system; Rust does not.

use super::*;

/// //Collect effect annotation hints at function definition sites.
///
/// Shows observed effects (throw, exit, register access, etc.)
/// next to function names. Uses pre-computed transitive effects
/// when available (from salsa-cached `transitive_effects`),
/// falling back to direct effects from the body.
///
/// Example: `function foo() = { throw ... }` → shows `/* throw */`
pub(super) fn collect_effect_hints(
    current_file: &dyn FileDb,
    begin: usize,
    end: usize,
    hints: &mut Vec<IdeDbInlayHint>,
    transitive_effects: Option<
        &std::collections::HashMap<String, std::collections::BTreeSet<hir_def::EffectTag>>,
    >,
) {
    let Some(bodies) = current_file.bodies() else {
        return;
    };

    for entry in bodies.entries() {
        let name_span = entry.def_span;
        if !span_starts_in_range(name_span, begin, end) {
            continue;
        }
        // Use pre-computed transitive effects if available,
        // else fall back to direct effects from body scan.
        let effects = transitive_effects
            .and_then(|te| te.get(&entry.name))
            .cloned()
            .unwrap_or_else(|| entry.effects.clone());
        if effects.is_empty() {
            continue;
        }
        let effect_strs: Vec<&str> = effects
            .iter()
            .filter(|e| !matches!(e, hir_def::EffectTag::Scattered | hir_def::EffectTag::NonExec))
            .map(|e| e.as_str())
            .collect();
        if effect_strs.is_empty() {
            continue;
        }
        let label = format!("/* {} */", effect_strs.join(", "));
        hints.push(IdeDbInlayHint {
            offset: name_span.end,
            label,
            kind: IdeDbInlayHintKind::Other,
            tooltip: Some(format!("Effects of `{}`: {}", entry.name, effect_strs.join(", "))),
            padding_left: Some(true),
            padding_right: Some(false),
            data: None,
        });
    }
}
