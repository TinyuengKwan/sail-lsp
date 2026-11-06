//! Render captures with precedence and binding-origin checks. Introduced fixed
//! binders are freshened; captured references must keep the same local owner.
use crate::{
    bindings::Bindings,
    fragments::FragmentKind,
    matching::Match,
    parsing::{Placeholder, Var},
    resolving::ResolvedRule,
};
use parser::SyntaxKind as SK;
use rowan::NodeOrToken;
use rustc_hash::{FxHashMap, FxHashSet};
use syntax::{SyntaxElement, SyntaxNode, SyntaxToken};

pub(crate) fn render_template(
    rule: &ResolvedRule,
    m: &Match,
    is_global: impl Fn(&hir::Name) -> bool,
) -> Option<String> {
    let template = &rule.template;
    let mut used: FxHashSet<_> = m
        .matched_node
        .ancestors()
        .last()?
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .map(|t| t.text().to_owned())
        .collect();
    used.extend(
        template
            .node
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .map(|t| t.text().to_owned()),
    );
    let mut renames = FxHashMap::default();
    if rule.kind != FragmentKind::Pattern {
        for (&binding, name) in &template.bindings.declarations {
            if template.placeholders_by_stand_in.contains_key(name) {
                continue;
            }
            let mut n = 0;
            let fresh = loop {
                let name = format!(
                    "{}sail_ssr_{}_{}",
                    if name.starts_with('\'') { "'" } else { "" },
                    name.trim_start_matches('\''),
                    n
                );
                if used.insert(name.clone()) {
                    break name;
                }
                n += 1;
            };
            renames.insert(binding, fresh.clone());
            for (&reference, target) in &template.bindings.references {
                if *target == Some(binding) {
                    renames.insert(reference, fresh.clone());
                }
            }
        }
    }
    let before = Bindings::of_with_globals(&m.matched_node, &is_global);
    let mut paired = FxHashMap::default();
    for (&binding, name) in &template.bindings.declarations {
        let matching: Vec<_> =
            rule.pattern.bindings.declarations.iter().filter(|(_, n)| *n == name).collect();
        if matching.len() == 1 {
            if let Some(actual) = m.token_mapping.get(matching[0].0) {
                paired.insert(binding, *actual);
            }
        }
    }
    let mut capture_renames = FxHashMap::default();
    for (&template_binding, &original) in &paired {
        if let Some(fresh) = renames.get(&template_binding) {
            for (&reference, target) in &before.references {
                if *target == Some(original) {
                    capture_renames.insert(reference, fresh.clone());
                }
            }
        }
    }
    let mut renderer = Renderer {
        output: String::new(),
        captures: &m.placeholder_values,
        placeholders: &template.placeholders_by_stand_in,
        renames,
        capture_renames,
        captured_origins: FxHashMap::default(),
        template_origins: FxHashMap::default(),
    };
    if rule.kind == FragmentKind::Expr {
        renderer.output.push('(');
    }
    renderer.node(&template.node);
    if rule.kind == FragmentKind::Expr {
        renderer.output.push(')');
    }
    let parsed = rule.kind.parse(&renderer.output)?;
    if rule.kind == FragmentKind::Item
        && syntax::definition_name(&m.matched_node) != syntax::definition_name(&parsed)
    {
        return None;
    }
    let after = Bindings::of_with_globals(&parsed, &is_global);
    if rule.kind == FragmentKind::Pattern {
        let mut a: Vec<_> = before.declarations.values().collect();
        a.sort();
        let mut b: Vec<_> = after.declarations.values().collect();
        b.sort();
        if a != b {
            return None;
        }
    }
    let offset = usize::from(parsed.text_range().start());
    for (&reference, target) in &after.references {
        let reference = reference.checked_sub(offset)?;
        let owner = target.and_then(|target| target.checked_sub(offset)).and_then(|target| {
            renderer.captured_origins.get(&target).copied().or_else(|| {
                renderer.template_origins.get(&target).and_then(|t| paired.get(t)).copied()
            })
        });
        if let Some(original) = renderer.captured_origins.get(&reference) {
            // A local owner inside the matched fragment must still exist at
            // the corresponding output occurrence. Outer bindings stay outer.
            let expected =
                before.references.get(original).copied().or_else(|| {
                    before.declarations.contains_key(original).then_some(Some(*original))
                });
            if let Some(expected) = expected {
                if expected != owner || (expected.is_none() && target.is_some()) {
                    return None;
                }
            }
        } else if let Some(original) = renderer.template_origins.get(&reference) {
            if template.bindings.references.get(original) == Some(&None) && target.is_some() {
                return None;
            }
        }
    }
    Some(renderer.output)
}

struct Renderer<'a> {
    output: String,
    captures: &'a FxHashMap<Var, SyntaxElement>,
    placeholders: &'a FxHashMap<String, Placeholder>,
    renames: FxHashMap<usize, String>,
    capture_renames: FxHashMap<usize, String>,
    captured_origins: FxHashMap<usize, usize>,
    template_origins: FxHashMap<usize, usize>,
}
impl Renderer<'_> {
    fn capture(&mut self, capture: &SyntaxElement, expression: bool) {
        if expression {
            self.output.push('(');
        }
        let tokens: Vec<_> = match capture {
            NodeOrToken::Node(node) => {
                node.descendants_with_tokens().filter_map(|e| e.into_token()).collect()
            }
            NodeOrToken::Token(token) => vec![token.clone()],
        };
        for token in tokens {
            let old = usize::from(token.text_range().start());
            if matches!(token.kind(), SK::IDENT | SK::TY_VAR) {
                self.captured_origins.insert(self.output.len(), old);
            }
            self.output
                .push_str(self.capture_renames.get(&old).map_or(token.text(), String::as_str));
        }
        if expression {
            self.output.push(')');
        }
    }
    fn token(&mut self, token: &SyntaxToken) {
        if let Some(capture) =
            self.placeholders.get(token.text()).and_then(|p| self.captures.get(&p.ident)).cloned()
        {
            self.capture(&capture, false);
            return;
        }
        let start = usize::from(token.text_range().start());
        if matches!(token.kind(), SK::IDENT | SK::TY_VAR) {
            self.template_origins.insert(self.output.len(), start);
        }
        self.output.push_str(self.renames.get(&start).map_or(token.text(), String::as_str));
    }
    fn node(&mut self, node: &SyntaxNode) {
        let elements: Vec<_> =
            node.children_with_tokens().filter(|e| !e.kind().is_trivia()).collect();
        if let [NodeOrToken::Token(token)] = elements.as_slice() {
            if let Some(capture) = self
                .placeholders
                .get(token.text())
                .and_then(|p| self.captures.get(&p.ident))
                .cloned()
            {
                self.capture(&capture, ide_db::syntax_helpers::node_ext::is_expr_kind(node.kind()));
                return;
            }
        }
        for element in node.children_with_tokens() {
            match element {
                NodeOrToken::Token(token) => self.token(&token),
                NodeOrToken::Node(node) => self.node(&node),
            }
        }
    }
}
