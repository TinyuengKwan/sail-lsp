//! Template-based replacement for SSR matches.
//!
//! Renders the replacement template with captured placeholder values
//! substituted in.

use parser::SyntaxKind;
use rowan::NodeOrToken;
use rustc_hash::FxHashMap;
use syntax::SyntaxNode;

use crate::matching::Match;
use crate::parsing::{Var, PLACEHOLDER_PREFIX};
use crate::resolving::ResolvedRule;

/// Render the replacement template for a match, substituting
/// placeholder captures from an AST-level match.
///
/// replaces placeholder nodes/tokens with captured text.
pub(crate) fn render_template(rule: &ResolvedRule, m: &Match) -> String {
    let template = &rule.template;
    let mut output = String::new();
    render_node(
        &template.node,
        &m.placeholder_values,
        &template.placeholders_by_stand_in,
        &mut output,
    );
    format!("({output})")
}

/// Recursively render a template node, substituting placeholders.
fn render_node(
    node: &SyntaxNode,
    captures: &FxHashMap<Var, SyntaxNode>,
    placeholders: &FxHashMap<String, crate::parsing::Placeholder>,
    output: &mut String,
) {
    // Check if this node is itself a placeholder wrapper.
    if let Some(ph) = get_placeholder_for_node(node, placeholders) {
        if let Some(captured) = captures.get(&ph.ident) {
            output.push_str(&format!("({})", captured.text()));
            return;
        }
    }

    for elem in node.children_with_tokens() {
        match elem {
            NodeOrToken::Token(tok) => {
                if tok.kind() == SyntaxKind::IDENT && tok.text().starts_with(PLACEHOLDER_PREFIX) {
                    // Token-level placeholder.
                    if let Some(ph) = placeholders.get(tok.text()) {
                        if let Some(captured) = captures.get(&ph.ident) {
                            output.push_str(&format!("({})", captured.text()));
                            continue;
                        }
                    }
                }
                output.push_str(tok.text());
            }
            NodeOrToken::Node(child) => {
                render_node(&child, captures, placeholders, output);
            }
        }
    }
}

/// Check if a node is a placeholder wrapper.
fn get_placeholder_for_node<'a>(
    node: &SyntaxNode,
    placeholders: &'a FxHashMap<String, crate::parsing::Placeholder>,
) -> Option<&'a crate::parsing::Placeholder> {
    let mut non_trivia =
        node.children_with_tokens().filter(|e| !e.as_token().is_some_and(|t| t.kind().is_trivia()));
    let first = non_trivia.next()?;
    if non_trivia.next().is_some() {
        return None;
    }
    let token = first.as_token()?;
    if token.kind() == SyntaxKind::IDENT && token.text().starts_with(PLACEHOLDER_PREFIX) {
        placeholders.get(token.text())
    } else {
        None
    }
}
