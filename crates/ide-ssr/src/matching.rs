//! AST-level structural matching.
//!
//! Two-phase matching: cheap structural check → recording of
//! placeholder bindings.

use parser::SyntaxKind;
use rowan::NodeOrToken;
use rustc_hash::FxHashMap;
use syntax::{SyntaxNode, SyntaxToken};

use hir_def::in_file::FileRange;

use crate::parsing::{Constraint, NodeKind, Placeholder, Var, PLACEHOLDER_PREFIX};
use crate::resolving::SyntaxNodeKey;

/// A comment token in a match (preserved for replacement).
///
/// with kind LINE_COMMENT, BLOCK_COMMENT, or DOC_COMMENT.
pub(crate) type Comment = syntax::SyntaxToken;

/// A single match found by AST-level SSR.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Match {
    /// Location of the matched node.
    pub(crate) range: FileRange,
    /// The matched syntax node.
    pub(crate) matched_node: SyntaxNode,
    /// Captured placeholder values.
    pub(crate) placeholder_values: FxHashMap<Var, PlaceholderMatch>,
    /// Comments inside the matched node that should be preserved in replacement.
    pub(crate) ignored_comments: Vec<Comment>,
    /// Paths in the replacement template pre-rendered for the match context.
    pub(crate) rendered_template_paths: FxHashMap<SyntaxNodeKey, String>,
    /// Which rule (by index) produced this match.
    pub(crate) rule_index: usize,
    /// Nesting depth (for overlap resolution).
    pub(crate) depth: usize,
}

/// A single placeholder's captured value.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct PlaceholderMatch {
    /// Location of the captured code.
    pub(crate) range: FileRange,
    /// The captured syntax node (or token wrapped in a node).
    pub(crate) node: SyntaxNode,
    /// Matches found within the captured node (for nested patterns).
    pub(crate) inner_matches: SsrMatches,
}

/// Collection of matches.
#[derive(Debug, Clone, Default)]
pub struct SsrMatches {
    pub(crate) matches: Vec<Match>,
}

impl SsrMatches {
    /// Flatten nested matches into a single list.
    pub fn flattened(self) -> impl Iterator<Item = Match> {
        self.matches.into_iter().flat_map(|m| {
            let inner: Vec<Match> = m
                .placeholder_values
                .values()
                .flat_map(|pm| pm.inner_matches.clone().flattened())
                .collect();
            std::iter::once(m).chain(inner)
        })
    }
}

impl Match {
    /// Return the text of the matched node.
    pub fn matched_text(&self) -> String {
        self.matched_node.text().to_string()
    }
}

/// Result of a failed match attempt.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct MatchFailed {
    pub(crate) reason: Option<String>,
}

impl MatchFailed {
    fn new(reason: impl Into<String>) -> Self {
        Self { reason: Some(reason.into()) }
    }
}

/// CST-level matcher.
#[allow(dead_code)]
pub(crate) struct Matcher {
    /// Placeholders keyed by stand-in name.
    pub(crate) placeholders: FxHashMap<String, Placeholder>,
    /// Rule index for tagging matches.
    pub(crate) rule_index: usize,
}

impl Matcher {
    /// Create a matcher from placeholder metadata.
    pub(crate) fn new(placeholders: FxHashMap<String, Placeholder>, rule_index: usize) -> Self {
        Self { placeholders, rule_index }
    }

    /// Attempt to match `code` node against `pattern` node.
    ///
    /// Returns captured placeholder bindings on success.
    pub(crate) fn try_match(
        &self,
        pattern: &SyntaxNode,
        code: &SyntaxNode,
    ) -> Result<FxHashMap<Var, PlaceholderMatch>, MatchFailed> {
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
        captures: &mut FxHashMap<Var, PlaceholderMatch>,
    ) -> Result<(), MatchFailed> {
        // Check if the entire pattern node is a placeholder wrapper.
        if let Some(placeholder) = self.get_placeholder_for_node(pattern) {
            return self.attempt_match_placeholder(&placeholder, code, captures);
        }

        // Node kinds must match.
        if pattern.kind() != code.kind() {
            return Err(MatchFailed::new(format!(
                "Node kind mismatch: pattern {:?} vs code {:?}",
                pattern.kind(),
                code.kind()
            )));
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
                    return Err(MatchFailed::new("Code has fewer children than pattern"));
                }
                (None, Some(_)) => {
                    return Err(MatchFailed::new("Code has more children than pattern"));
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
                                return Err(MatchFailed::new(
                                    "Pattern token vs code node mismatch",
                                ));
                            }
                        }
                        // Pattern is node, code is token — check if pattern is placeholder wrapper.
                        (NodeOrToken::Node(pn), NodeOrToken::Token(_ct)) => {
                            if self.get_placeholder_for_node(&pn).is_some() {
                                // Placeholder matching a token — bind the parent node.
                                // We need the code *node* but we only have a token.
                                // This case is unusual; fail gracefully.
                                return Err(MatchFailed::new(
                                    "Placeholder node vs code token mismatch",
                                ));
                            }
                            return Err(MatchFailed::new("Pattern node vs code token mismatch"));
                        }
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
        captures: &mut FxHashMap<Var, PlaceholderMatch>,
    ) -> Result<(), MatchFailed> {
        // Check if pattern token is a placeholder.
        if let Some(placeholder) = self.get_placeholder_for_token(pattern_token) {
            let range = FileRange::from(code_token.text_range());
            let node = code_token.parent().unwrap_or_else(|| {
                // Fallback: shouldn't happen in practice.
                code_token.parent().unwrap()
            });
            let pm = PlaceholderMatch { range, node, inner_matches: SsrMatches::default() };
            return self.record_binding(&placeholder, pm, captures);
        }

        // Exact text match for literal tokens.
        if pattern_token.kind() != code_token.kind() {
            return Err(MatchFailed::new(format!(
                "Token kind mismatch: {:?} vs {:?}",
                pattern_token.kind(),
                code_token.kind()
            )));
        }
        if pattern_token.text() != code_token.text() {
            return Err(MatchFailed::new(format!(
                "Token text mismatch: {:?} vs {:?}",
                pattern_token.text(),
                code_token.text()
            )));
        }
        Ok(())
    }

    /// Bind a placeholder to a code node.
    fn attempt_match_placeholder(
        &self,
        placeholder: &Placeholder,
        code: &SyntaxNode,
        captures: &mut FxHashMap<Var, PlaceholderMatch>,
    ) -> Result<(), MatchFailed> {
        // Check constraints.
        if !self.check_constraints(&placeholder.constraints, code) {
            return Err(MatchFailed::new(format!(
                "Constraint check failed for placeholder ${}",
                placeholder.ident.0
            )));
        }

        let range = FileRange::from(code.text_range());
        let pm =
            PlaceholderMatch { range, node: code.clone(), inner_matches: SsrMatches::default() };
        self.record_binding(placeholder, pm, captures)
    }

    /// Record a placeholder binding, checking consistency.
    fn record_binding(
        &self,
        placeholder: &Placeholder,
        new_match: PlaceholderMatch,
        captures: &mut FxHashMap<Var, PlaceholderMatch>,
    ) -> Result<(), MatchFailed> {
        let var = placeholder.ident.clone();
        if let Some(existing) = captures.get(&var) {
            // Same placeholder must match same text.
            if existing.node.text() != new_match.node.text() {
                return Err(MatchFailed::new(format!(
                    "Inconsistent binding for ${}: {:?} vs {:?}",
                    var.0,
                    existing.node.text(),
                    new_match.node.text()
                )));
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
