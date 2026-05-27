//! Semantic resolution for SSR rules.
//! For Sail: path resolution means resolving identifiers to their
//! definitions (functions, types, registers). Two paths match if they
//! resolve to the same definition.
//!
//! Currently a stub — always returns "no resolution". Will be filled in
//! once the SSR infrastructure is integrated with actual semantic queries.

use rustc_hash::FxHashMap;

use syntax::SyntaxNode;

use crate::parsing::{ParsedRule, Placeholder};

/// A fully resolved SSR rule, ready for matching.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedRule {
    /// The resolved pattern (search side).
    pub(crate) pattern: ResolvedPattern,
    /// The resolved template (replacement side), if present.
    pub(crate) template: Option<ResolvedPattern>,
    /// Index of this rule in the MatchFinder's rule list.
    pub(crate) index: usize,
}

/// A pattern with resolved path information.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct ResolvedPattern {
    /// Placeholder metadata, keyed by stand-in name.
    pub(crate) placeholders_by_stand_in: FxHashMap<String, Placeholder>,
    /// The parsed syntax node for this pattern.
    pub(crate) node: SyntaxNode,
    /// Path nodes that resolved to a definition.
    /// Currently empty — stub for future semantic matching.
    pub(crate) resolved_paths: FxHashMap<SyntaxNodeKey, ResolvedPath>,
}

/// A resolved path — points to a definition.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct ResolvedPath {
    /// The semantic resolution of this path.
    pub(crate) resolution: hir::PathResolution,
    /// Depth of this path in the AST (for resolution priority).
    pub(crate) depth: u32,
}

/// Key for SyntaxNode in HashMap (by text range, since SyntaxNode doesn't impl Hash).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[allow(dead_code)]
pub(crate) struct SyntaxNodeKey {
    range: rowan::TextRange,
    kind: parser::SyntaxKind,
}

impl SyntaxNodeKey {
    #[allow(dead_code)]
    pub(crate) fn from_node(node: &SyntaxNode) -> Self {
        Self { range: node.text_range(), kind: node.kind() }
    }
}

/// Scope in which resolution happens.
pub(crate) struct ResolutionScope<'db> {
    pub(crate) _phantom: std::marker::PhantomData<&'db ()>,
}

impl<'db> ResolutionScope<'db> {
    pub(crate) fn new(_sema: &'db hir::Semantics<'db>) -> Self {
        Self { _phantom: std::marker::PhantomData }
    }

    /// Resolve a parsed rule into a resolved rule.
    ///
    /// Currently a stub: does not perform actual path resolution.
    /// The resolved_paths map will be empty.
    pub(crate) fn resolve_rule(&self, parsed: &ParsedRule, index: usize) -> ResolvedRule {
        let pattern = self.resolve_pattern(&parsed.pattern, &parsed.placeholders_by_stand_in);
        let template = parsed
            .template
            .as_ref()
            .map(|t| self.resolve_pattern(t, &parsed.placeholders_by_stand_in));
        ResolvedRule { pattern, template, index }
    }

    /// Resolve paths within a pattern node.
    ///
    /// Stub: returns empty resolved_paths. Future: walk the tree, find
    /// path/name nodes, use sema.resolve_path() to get definitions.
    fn resolve_pattern(
        &self,
        node: &SyntaxNode,
        placeholders: &FxHashMap<String, Placeholder>,
    ) -> ResolvedPattern {
        let _ = &self._phantom; // Will use sema for actual resolution later.
        ResolvedPattern {
            placeholders_by_stand_in: placeholders.clone(),
            node: node.clone(),
            resolved_paths: FxHashMap::default(),
        }
    }
}
