//! AST-level structural matching.
//!
//! Two-phase matching: cheap structural check → recording of
//! placeholder bindings.

use parser::SyntaxKind;
use rowan::NodeOrToken;
use rustc_hash::FxHashMap;
use syntax::{SyntaxNode, SyntaxToken};

use crate::parsing::{Constraint, NodeKind, Placeholder, Var, PLACEHOLDER_PREFIX};

/// A single match found by AST-level SSR.
#[derive(Debug, Clone)]
pub(crate) struct Match {
    /// The matched syntax node.
    pub(crate) matched_node: SyntaxNode,
    /// Captured placeholder values.
    pub(crate) placeholder_values: FxHashMap<Var, SyntaxNode>,
}

/// CST-level matcher.
pub(crate) struct Matcher<'a> {
    /// Placeholders keyed by stand-in name.
    pub(crate) placeholders: &'a FxHashMap<String, Placeholder>,
}

impl<'a> Matcher<'a> {
    pub(crate) fn new(placeholders: &'a FxHashMap<String, Placeholder>) -> Self {
        Self { placeholders }
    }

    /// Attempt to match `code` node against `pattern` node.
    ///
    /// Returns captured placeholder bindings on success.
    pub(crate) fn try_match(
        &self,
        pattern: &SyntaxNode,
        code: &SyntaxNode,
    ) -> Result<FxHashMap<Var, SyntaxNode>, ()> {
        let mut captures = FxHashMap::default();
        self.attempt_match_node(pattern, code, &mut captures)?;
        Ok(captures)
    }

    /// Recursive node matching.
    ///
    /// Algorithm:
    /// 1. Skip trivia on both sides
    /// 2. If pattern element is a placeholder → bind it
    /// 3. If pattern element is a regular node → check kind matches, recurse on children
    /// 4. If pattern element is a token → exact text match
    fn attempt_match_node(
        &self,
        pattern: &SyntaxNode,
        code: &SyntaxNode,
        captures: &mut FxHashMap<Var, SyntaxNode>,
    ) -> Result<(), ()> {
        // Check if the entire pattern node is a placeholder wrapper.
        if let Some(placeholder) = self.get_placeholder_for_node(pattern) {
            return self.attempt_match_placeholder(&placeholder, code, captures);
        }

        // Node kinds must match.
        if pattern.kind() != code.kind() {
            return Err(());
        }

        // Iterate children in parallel, skipping trivia.
        let mut pat_children = pattern.children_with_tokens().peekable();
        let mut code_children = code.children_with_tokens().peekable();

        loop {
            skip_trivia(&mut pat_children);
            skip_trivia(&mut code_children);

            match (pat_children.peek(), code_children.peek()) {
                (None, None) => return Ok(()),
                (Some(_), None) => {
                    return Err(());
                }
                (None, Some(_)) => {
                    return Err(());
                }
                (Some(pat_elem), Some(code_elem)) => {
                    match (pat_elem.clone(), code_elem.clone()) {
                        // Both tokens.
                        (NodeOrToken::Token(pt), NodeOrToken::Token(ct)) => {
                            self.attempt_match_token(&pt, &ct, captures)?;
                            pat_children.next();
                            code_children.next();
                        }
                        // Both nodes.
                        (NodeOrToken::Node(pn), NodeOrToken::Node(cn)) => {
                            self.attempt_match_node(&pn, &cn, captures)?;
                            pat_children.next();
                            code_children.next();
                        }
                        // Pattern is token, code is node — check if pattern token is placeholder.
                        (NodeOrToken::Token(pt), NodeOrToken::Node(cn)) => {
                            if let Some(placeholder) = self.get_placeholder_for_token(&pt) {
                                self.attempt_match_placeholder(&placeholder, &cn, captures)?;
                                pat_children.next();
                                code_children.next();
                            } else {
                                return Err(());
                            }
                        }
                        // A captured expression needs a syntax node.
                        (NodeOrToken::Node(_), NodeOrToken::Token(_)) => return Err(()),
                    }
                }
            }
        }
    }

    /// Match a pattern token against a code token.
    fn attempt_match_token(
        &self,
        pattern_token: &SyntaxToken,
        code_token: &SyntaxToken,
        captures: &mut FxHashMap<Var, SyntaxNode>,
    ) -> Result<(), ()> {
        // Check if pattern token is a placeholder.
        if let Some(placeholder) = self.get_placeholder_for_token(pattern_token) {
            let node = code_token.parent().ok_or(())?;
            let pm = node;
            return self.record_binding(&placeholder, pm, captures);
        }

        // Exact text match for literal tokens.
        if pattern_token.kind() != code_token.kind() {
            return Err(());
        }
        if pattern_token.text() != code_token.text() {
            return Err(());
        }
        Ok(())
    }

    /// Bind a placeholder to a code node.
    fn attempt_match_placeholder(
        &self,
        placeholder: &Placeholder,
        code: &SyntaxNode,
        captures: &mut FxHashMap<Var, SyntaxNode>,
    ) -> Result<(), ()> {
        // Check constraints.
        if !self.check_constraints(&placeholder.constraints, code) {
            return Err(());
        }

        let pm = code.clone();
        self.record_binding(placeholder, pm, captures)
    }

    /// Record a placeholder binding, checking consistency.
    fn record_binding(
        &self,
        placeholder: &Placeholder,
        new_match: SyntaxNode,
        captures: &mut FxHashMap<Var, SyntaxNode>,
    ) -> Result<(), ()> {
        let var = placeholder.ident.clone();
        if let Some(existing) = captures.get(&var) {
            // Same placeholder must match same text.
            if existing.text() != new_match.text() {
                return Err(());
            }
        } else {
            captures.insert(var, new_match);
        }
        Ok(())
    }

    /// Check if a code node satisfies placeholder constraints.
    fn check_constraints(&self, constraints: &[Constraint], node: &SyntaxNode) -> bool {
        constraints.iter().all(|c| self.check_single_constraint(c, node))
    }

    fn check_single_constraint(&self, constraint: &Constraint, node: &SyntaxNode) -> bool {
        match constraint {
            Constraint::Kind(kind) => match kind {
                NodeKind::Literal => {
                    node.kind() == SyntaxKind::LITERAL_EXPR
                        || node.kind() == SyntaxKind::LITERAL_PAT
                }
            },
            Constraint::Not(inner) => !self.check_single_constraint(inner, node),
        }
    }

    /// Check if a token is a placeholder (starts with `__ssr_` prefix).
    fn get_placeholder_for_token(&self, token: &SyntaxToken) -> Option<Placeholder> {
        if token.kind() == SyntaxKind::IDENT && token.text().starts_with(PLACEHOLDER_PREFIX) {
            self.placeholders.get(token.text()).cloned()
        } else {
            None
        }
    }

    /// Check if a node is a placeholder wrapper (single `__ssr_*` token child).
    fn get_placeholder_for_node(&self, node: &SyntaxNode) -> Option<Placeholder> {
        let mut non_trivia = node
            .children_with_tokens()
            .filter(|e| !e.as_token().is_some_and(|t| t.kind().is_trivia()));
        let first = non_trivia.next()?;
        if non_trivia.next().is_some() {
            return None;
        }
        let token = first.as_token()?;
        if token.kind() == SyntaxKind::IDENT && token.text().starts_with(PLACEHOLDER_PREFIX) {
            self.placeholders.get(token.text()).cloned()
        } else {
            None
        }
    }
}

/// Skip whitespace and comment tokens in the iterator.
fn skip_trivia(
    iter: &mut std::iter::Peekable<impl Iterator<Item = NodeOrToken<SyntaxNode, SyntaxToken>>>,
) {
    while let Some(elem) = iter.peek() {
        let is_trivia = match elem {
            NodeOrToken::Token(tok) => tok.kind().is_trivia(),
            NodeOrToken::Node(_) => false,
        };
        if is_trivia {
            iter.next();
        } else {
            break;
        }
    }
}
