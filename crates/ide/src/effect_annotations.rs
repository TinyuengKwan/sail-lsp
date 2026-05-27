//! Effect flow annotations — code lens showing per-function effects.
//!
//! Sail-specific feature: shows inferred effects as code lens above
//! each function definition. Uses `transitive_effects` from
//! hir-def for accurate cross-function effect propagation.
//!
//! Example:
//! ```text
//! [effects: throw, wreg]        ← code lens
//! function execute(instr) = {
//! }
//! ```

use hir_def::bodies::EffectTag;
use ide_db::FileDb;
use std::collections::BTreeSet;

/// An effect annotation for a single callable.
#[derive(Debug, Clone)]
pub struct EffectAnnotation {
    /// The callable's name.
    pub name: String,
    /// Byte span of the definition (for positioning the lens).
    pub span: parser::Span,
    /// Inferred effects (direct + transitive).
    pub effects: BTreeSet<EffectTag>,
}

/// Collect effect annotations for all callables in a file.
///
/// Returns one annotation per callable that has non-empty effects.
/// Suitable for rendering as code lenses.
pub fn effect_annotations(file: &dyn FileDb) -> Vec<EffectAnnotation> {
    let text = file.text();
    if text.is_empty() {
        return Vec::new();
    }

    let (cst_root, _) = syntax::parse_text(text);
    let bodies = hir_def::bodies::CallableBodies::from_cst(&cst_root);
    let callgraph = hir_def::callgraph::CallGraph::from_callable_bodies(&bodies);

    // Collect direct effects
    let mut effects_map: std::collections::HashMap<String, BTreeSet<EffectTag>> =
        std::collections::HashMap::new();
    for entry in bodies.entries() {
        effects_map.insert(entry.name.clone(), entry.effects.clone());
    }

    // Transitive propagation via callgraph (fixed-point)
    let max_iters = effects_map.len() + 1;
    for _ in 0..max_iters {
        let mut changed = false;
        let snapshot: Vec<(String, BTreeSet<EffectTag>)> =
            effects_map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

        for (name, current) in &snapshot {
            let callee_effects: BTreeSet<EffectTag> = callgraph
                .callees_of(name)
                .flat_map(|callee| {
                    snapshot
                        .iter()
                        .find(|(n, _)| n == callee)
                        .map(|(_, e)| e.iter().copied())
                        .into_iter()
                        .flatten()
                })
                .collect();

            let merged: BTreeSet<EffectTag> = current.union(&callee_effects).copied().collect();
            if merged.len() > current.len() {
                effects_map.insert(name.clone(), merged);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Build annotations for non-empty effect sets
    let mut annotations = Vec::new();
    for entry in bodies.entries() {
        if let Some(effects) = effects_map.get(&entry.name) {
            if !effects.is_empty() {
                annotations.push(EffectAnnotation {
                    name: entry.name.clone(),
                    span: entry.def_span,
                    effects: effects.clone(),
                });
            }
        }
    }

    annotations
}

/// Format an effect set as a display string: `{throw, wreg}`.
pub fn format_effects(effects: &BTreeSet<EffectTag>) -> String {
    if effects.is_empty() {
        return "pure".to_string();
    }
    let names: Vec<&str> = effects.iter().map(|e| e.as_str()).collect();
    format!("{{{}}}", names.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_db::test_utils::TestFile;

    #[test]
    fn detect_throw_effect() {
        let file = TestFile::new("function f() = throw(\"error\")\n");
        let annotations = effect_annotations(&file);
        assert_eq!(annotations.len(), 1);
        assert!(annotations[0].effects.contains(&EffectTag::Throw));
    }

    #[test]
    fn pure_function_no_annotation() {
        let file = TestFile::new("function g(x) = x + 1\n");
        let annotations = effect_annotations(&file);
        // Pure functions have no effects → no annotations
        assert!(
            annotations.is_empty() || annotations[0].effects.is_empty(),
            "pure function should have no effect annotations"
        );
    }

    #[test]
    fn format_effects_display() {
        let mut effects = BTreeSet::new();
        effects.insert(EffectTag::Throw);
        effects.insert(EffectTag::RegisterWrite);
        assert_eq!(format_effects(&effects), "{throw, wreg}");
    }
}
