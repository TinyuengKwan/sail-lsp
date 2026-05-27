//! Transitive effect propagation via call graph.
//!
//! `effect_of(f) = direct_effects(f) ∪ ⋃{ effect_of(g) | f calls g }`

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use crate::bodies::{CallableBodies, EffectTag};
use crate::callgraph::CallGraph;

/// Per-function computed effects (direct + transitive from callees).
#[derive(Debug, Clone, Default)]
pub struct FunctionEffects {
    pub direct: BTreeSet<EffectTag>,
    pub transitive: BTreeSet<EffectTag>,
    pub combined: BTreeSet<EffectTag>,
    pub outcomes: BTreeSet<String>,
}

impl FunctionEffects {
    pub fn is_pure(&self) -> bool {
        self.combined.is_empty() && self.outcomes.is_empty()
    }

    pub fn has_effect(&self, tag: EffectTag) -> bool {
        self.combined.contains(&tag)
    }

    /// Check if this function invokes a specific outcome.
    pub fn has_outcome(&self, name: &str) -> bool {
        self.outcomes.contains(name)
    }
}

/// Workspace-wide effect map: function name -> computed effects.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceEffects {
    effects: HashMap<String, FunctionEffects>,
}

impl WorkspaceEffects {
    /// Get the computed effects for a function.
    pub fn get(&self, name: &str) -> Option<&FunctionEffects> {
        self.effects.get(name)
    }

    /// Check if a function is pure (no effects at all).
    /// Unknown functions are assumed pure (conservative for diagnostics).
    pub fn is_pure(&self, name: &str) -> bool {
        self.effects.get(name).map(|e| e.is_pure()).unwrap_or(true)
    }

    /// Get the combined effect set for a function.
    pub fn combined_effects(&self, name: &str) -> BTreeSet<EffectTag> {
        self.effects.get(name).map(|e| e.combined.clone()).unwrap_or_default()
    }

    /// Number of functions with computed effects.
    pub fn len(&self) -> usize {
        self.effects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    /// Iterate all function effects.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &FunctionEffects)> {
        self.effects.iter()
    }
}

/// Compute transitive effects for a single function by BFS over callees.
pub fn compute_transitive_effects(
    name: &str,
    direct_effects: &HashMap<String, BTreeSet<EffectTag>>,
    direct_outcomes: &HashMap<String, BTreeSet<String>>,
    callgraph: &CallGraph,
) -> FunctionEffects {
    let direct = direct_effects.get(name).cloned().unwrap_or_default();
    let own_outcomes = direct_outcomes.get(name).cloned().unwrap_or_default();
    let mut transitive = BTreeSet::new();
    let mut outcomes = own_outcomes.clone();
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();

    visited.insert(name.to_string());

    // Seed with direct callees
    for site in callgraph.call_sites_in(name) {
        if !visited.contains(&site.callee) {
            visited.insert(site.callee.clone());
            queue.push_back(site.callee.clone());
        }
    }

    // BFS: propagate effects + outcomes from all reachable callees
    while let Some(callee) = queue.pop_front() {
        // Add callee's direct effects to our transitive set
        if let Some(callee_effects) = direct_effects.get(&callee) {
            transitive.extend(callee_effects.iter().copied());
        }

        // Propagate callee's outcomes
        if let Some(callee_outcomes) = direct_outcomes.get(&callee) {
            outcomes.extend(callee_outcomes.iter().cloned());
        }

        // Enqueue callee's callees
        for site in callgraph.call_sites_in(&callee) {
            if !visited.contains(&site.callee) {
                visited.insert(site.callee.clone());
                queue.push_back(site.callee.clone());
            }
        }
    }

    let mut combined = direct.clone();
    combined.extend(transitive.iter().copied());

    FunctionEffects { direct, transitive, combined, outcomes }
}

/// Batch-compute transitive effects for all functions in a workspace.
pub fn compute_workspace_effects(
    all_bodies: &[&CallableBodies],
    callgraph: &CallGraph,
) -> WorkspaceEffects {
    // Collect direct effects + outcomes per function name
    let mut direct_effects: HashMap<String, BTreeSet<EffectTag>> = HashMap::new();
    let mut direct_outcomes: HashMap<String, BTreeSet<String>> = HashMap::new();
    for bodies in all_bodies {
        for entry in bodies.entries() {
            direct_effects
                .entry(entry.name.clone())
                .or_default()
                .extend(entry.effects.iter().copied());
            if !entry.outcomes.is_empty() {
                direct_outcomes
                    .entry(entry.name.clone())
                    .or_default()
                    .extend(entry.outcomes.iter().cloned());
            }
        }
    }

    // Compute transitive effects + outcomes for each function
    let mut effects = HashMap::new();
    for name in direct_effects.keys() {
        let fe = compute_transitive_effects(name, &direct_effects, &direct_outcomes, callgraph);
        effects.insert(name.clone(), fe);
    }

    WorkspaceEffects { effects }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::callgraph::CallGraph;
    use crate::Span;

    fn make_callgraph(edges: &[(&str, &str)]) -> CallGraph {
        use crate::callgraph::CallSite;
        let sites: Vec<CallSite> = edges
            .iter()
            .map(|(caller, callee)| CallSite {
                caller: caller.to_string(),
                callee: callee.to_string(),
                callee_span: Span::new(0, 0),
            })
            .collect();
        CallGraph::from_sites(sites)
    }

    fn direct(tags: &[EffectTag]) -> BTreeSet<EffectTag> {
        tags.iter().copied().collect()
    }

    #[test]
    fn pure_function_has_no_effects() {
        let direct_effects: HashMap<String, BTreeSet<EffectTag>> =
            HashMap::from([("f".to_string(), BTreeSet::new())]);
        let cg = make_callgraph(&[]);

        let fe = compute_transitive_effects("f", &direct_effects, &HashMap::new(), &cg);
        assert!(fe.is_pure());
        assert!(fe.direct.is_empty());
        assert!(fe.transitive.is_empty());
    }

    #[test]
    fn direct_effects_propagated() {
        let direct_effects: HashMap<String, BTreeSet<EffectTag>> =
            HashMap::from([("f".to_string(), direct(&[EffectTag::Throw, EffectTag::Exit]))]);
        let cg = make_callgraph(&[]);

        let fe = compute_transitive_effects("f", &direct_effects, &HashMap::new(), &cg);
        assert!(!fe.is_pure());
        assert!(fe.has_effect(EffectTag::Throw));
        assert!(fe.has_effect(EffectTag::Exit));
        assert!(fe.transitive.is_empty());
    }

    #[test]
    fn transitive_effects_from_callee() {
        // f calls g, g has Throw effect
        let direct_effects: HashMap<String, BTreeSet<EffectTag>> = HashMap::from([
            ("f".to_string(), BTreeSet::new()),
            ("g".to_string(), direct(&[EffectTag::Throw])),
        ]);
        let cg = make_callgraph(&[("f", "g")]);

        let fe = compute_transitive_effects("f", &direct_effects, &HashMap::new(), &cg);
        assert!(fe.direct.is_empty()); // f has no direct effects
        assert!(fe.has_effect(EffectTag::Throw)); // but inherits from g
        assert!(fe.transitive.contains(&EffectTag::Throw));
    }

    #[test]
    fn transitive_chain_a_calls_b_calls_c() {
        // a → b → c, c has Exit
        let direct_effects: HashMap<String, BTreeSet<EffectTag>> = HashMap::from([
            ("a".to_string(), BTreeSet::new()),
            ("b".to_string(), BTreeSet::new()),
            ("c".to_string(), direct(&[EffectTag::Exit])),
        ]);
        let cg = make_callgraph(&[("a", "b"), ("b", "c")]);

        let fe = compute_transitive_effects("a", &direct_effects, &HashMap::new(), &cg);
        assert!(fe.has_effect(EffectTag::Exit)); // propagated through b
    }

    #[test]
    fn cycle_detection_terminates() {
        // a → b → a (cycle)
        let direct_effects: HashMap<String, BTreeSet<EffectTag>> = HashMap::from([
            ("a".to_string(), direct(&[EffectTag::Return])),
            ("b".to_string(), direct(&[EffectTag::Throw])),
        ]);
        let cg = make_callgraph(&[("a", "b"), ("b", "a")]);

        let fe = compute_transitive_effects("a", &direct_effects, &HashMap::new(), &cg);
        // Should not loop infinitely
        assert!(fe.has_effect(EffectTag::Return)); // direct
        assert!(fe.has_effect(EffectTag::Throw)); // from b
    }

    #[test]
    fn workspace_effects_batch() {
        let direct_effects_f = direct(&[EffectTag::Assert]);
        let direct_effects_g = direct(&[EffectTag::Throw]);

        // Build CallableBodies manually for testing
        let bodies = CallableBodies::from_entries(vec![
            ("f".to_string(), Span::new(0, 10), direct_effects_f.clone()),
            ("g".to_string(), Span::new(20, 30), direct_effects_g.clone()),
        ]);

        let cg = make_callgraph(&[("f", "g")]);

        let we = compute_workspace_effects(&[&bodies], &cg);
        assert_eq!(we.len(), 2);
        assert!(we.get("f").unwrap().has_effect(EffectTag::Assert)); // direct
        assert!(we.get("f").unwrap().has_effect(EffectTag::Throw)); // transitive
        assert!(we.get("g").unwrap().has_effect(EffectTag::Throw)); // direct
        assert!(we.is_pure("nonexistent")); // unknown function → no effects
    }

    #[test]
    fn outcome_propagation() {
        // f calls g, g produces outcome "Error"
        let direct_effects: HashMap<String, BTreeSet<EffectTag>> = HashMap::from([
            ("f".to_string(), BTreeSet::new()),
            ("g".to_string(), direct(&[EffectTag::Throw])),
        ]);
        let direct_outcomes: HashMap<String, BTreeSet<String>> =
            HashMap::from([("g".to_string(), ["Error".to_string()].into_iter().collect())]);
        let cg = make_callgraph(&[("f", "g")]);

        let fe = compute_transitive_effects("f", &direct_effects, &direct_outcomes, &cg);
        assert!(fe.has_outcome("Error")); // propagated from g
        assert!(!fe.is_pure()); // has outcome → not pure
        assert!(fe.has_effect(EffectTag::Throw)); // effect also propagated
    }

    #[test]
    fn outcome_chain_propagation() {
        // a → b → c, c produces outcome "Timeout"
        let direct_effects: HashMap<String, BTreeSet<EffectTag>> = HashMap::from([
            ("a".to_string(), BTreeSet::new()),
            ("b".to_string(), BTreeSet::new()),
            ("c".to_string(), BTreeSet::new()),
        ]);
        let direct_outcomes: HashMap<String, BTreeSet<String>> =
            HashMap::from([("c".to_string(), ["Timeout".to_string()].into_iter().collect())]);
        let cg = make_callgraph(&[("a", "b"), ("b", "c")]);

        let fe = compute_transitive_effects("a", &direct_effects, &direct_outcomes, &cg);
        assert!(fe.has_outcome("Timeout")); // propagated through b → c
    }

    #[test]
    fn pure_function_no_outcomes() {
        let direct_effects: HashMap<String, BTreeSet<EffectTag>> =
            HashMap::from([("f".to_string(), BTreeSet::new())]);
        let cg = make_callgraph(&[]);

        let fe = compute_transitive_effects("f", &direct_effects, &HashMap::new(), &cg);
        assert!(fe.outcomes.is_empty());
        assert!(fe.is_pure());
    }
}
