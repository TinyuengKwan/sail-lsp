//! Structural Search Replace (SSR).
//!
//! # ALIGN (RA counterpart: `ide-ssr`)
//! - Near-identical clone: 10/11 files match RA's layout
//! - `SsrRule`, `SsrPattern`, `MatchFinder`, `SsrMatches` public API identical to RA
//! - `$name` placeholder syntax and `==>>` rule separator unchanged
//! - `parsing`, `fragments`, `matching`, `resolving`, `search`, `replacing`, `nester`,
//!   `errors`, `from_comment` modules all present and structurally aligned
//! - `MatchFinder::in_context` / `at_first_file` mirror RA constructor pattern
//! - `debug_where_text_equal` mirrors RA debug helper
//!
//! # CUSTOM (Sail-specific, no RA counterpart)
//! - `resolving.rs` is a stub: `ResolutionScope` not yet wired to `DefMap` path resolution
//! - `edits()` / `matches()` return empty until workspace-wide file iteration is plumbed in
//! - Tests are inlined in `lib.rs` rather than a separate `tests.rs` (only structural difference vs RA)
//!
//! Module layout:
//! - `lib.rs` — public API: `SsrRule`, `SsrPattern`, `MatchFinder`, `SsrMatches`
//! - `errors.rs` — `SsrError` + `bail!` macro
//! - `parsing.rs` — rule tokenization, placeholder extraction, constraint parsing
//! - `fragments.rs` — fragment parsing (expr, pat, typ, item)
//! - `matching.rs` — AST-level structural matching with placeholders
//! - `resolving.rs` — semantic path resolution (stub)
//! - `search.rs` — usage-index-accelerated candidate search
//! - `replacing.rs` — template rendering with placeholder substitution
//! - `nester.rs` — overlap deduplication
//! - `from_comment.rs` — assist integration (SSR from comments)
//!
//! Syntax: `search_pattern ==>> replace_pattern`
//! Example: `foo($a, $b) ==>> bar($b, $a)` swaps arguments.
//!
//! Placeholders:
//! - `$name` — matches any sub-expression/node
//! - `${name:kind(literal)}` — constrained placeholder

#[macro_use]
mod errors;
mod fragments;
mod from_comment;
mod matching;
mod nester;
mod parsing;
mod replacing;
mod resolving;
mod search;

use rustc_hash::FxHashMap;

use base_db::FileId;

pub use crate::errors::SsrError;
pub use crate::from_comment::ssr_from_comment;
pub use crate::matching::{Match, SsrMatches};

/// Debug information for a match attempt that failed.
#[derive(Debug)]
pub struct MatchDebugInfo {
    /// Why the match failed (empty if it succeeded).
    pub matched: Result<Match, String>,
}

use resolving::{ResolutionScope, ResolvedRule};

/// A parsed SSR rule (search + replace).
#[derive(Debug, Clone)]
pub struct SsrRule {
    /// The raw rule text.
    raw: String,
}

impl std::str::FromStr for SsrRule {
    type Err = SsrError;

    /// Parse an SSR rule from `search ==>> replace` syntax.
    fn from_str(input: &str) -> Result<Self, SsrError> {
        let parts: Vec<&str> = input.splitn(2, "==>>").collect();
        if parts.len() != 2 {
            bail!("Expected `search ==>> replace` syntax");
        }
        let search_str = parts[0].trim();
        let replace_str = parts[1].trim();
        if search_str.is_empty() {
            bail!("Search pattern is empty");
        }
        if replace_str.is_empty() {
            bail!("Replace pattern is empty");
        }
        Ok(SsrRule { raw: input.to_string() })
    }
}

/// A search-only SSR pattern (no template).
#[derive(Debug, Clone)]
pub struct SsrPattern {
    /// The raw pattern text.
    raw: String,
}

impl std::str::FromStr for SsrPattern {
    type Err = SsrError;

    /// Parse a search-only pattern.
    fn from_str(input: &str) -> Result<Self, SsrError> {
        if input.contains("==>>") {
            bail!("SsrPattern should not contain ==>> separator");
        }
        let trimmed = input.trim();
        if trimmed.is_empty() {
            bail!("Search pattern is empty");
        }
        Ok(SsrPattern { raw: input.to_string() })
    }
}

/// High-level SSR coordinator.
///
/// ```text
/// pub struct MatchFinder<'db> {
///     sema: Semantics<'db, RootDatabase>,
///     rules: Vec<ResolvedRule>,
///     resolution_scope: ResolutionScope<'db>,
/// }
/// ```
pub struct MatchFinder<'db> {
    /// Semantic analysis for path resolution.
    sema: hir::Semantics<'db>,
    /// Resolved rules (AST-level).
    rules: Vec<ResolvedRule>,
    /// Resolution scope (cached).
    #[allow(dead_code)]
    resolution_scope: ResolutionScope<'db>,
}

impl<'db> MatchFinder<'db> {
    /// Create a MatchFinder with semantic analysis context.
    pub fn in_context(
        db: &'db dyn salsa::Database,
        _ft: base_db::FileText,
    ) -> Result<Self, SsrError> {
        let sema = hir::Semantics::new(db);
        Ok(Self {
            sema,
            rules: Vec::new(),
            resolution_scope: ResolutionScope { _phantom: std::marker::PhantomData },
        })
    }

    /// Convenience constructor.
    pub fn at_first_file(
        db: &'db dyn salsa::Database,
        ft: base_db::FileText,
    ) -> Result<Self, SsrError> {
        Self::in_context(db, ft)
    }

    /// Add a rule from `search ==>> replace` syntax.
    pub fn add_rule(&mut self, rule: SsrRule) -> Result<(), SsrError> {
        let parsed_rules = parsing::parse_rules(&rule.raw)?;
        let scope = ResolutionScope::new(&self.sema);
        let base_index = self.rules.len();
        for (i, parsed) in parsed_rules.iter().enumerate() {
            let resolved = scope.resolve_rule(parsed, base_index + i);
            self.rules.push(resolved);
        }
        Ok(())
    }

    /// Add a search-only pattern (no replacement).
    pub fn add_search_pattern(&mut self, pattern: SsrPattern) -> Result<(), SsrError> {
        let parsed_rules = parsing::parse_pattern_only(&pattern.raw)?;
        let scope = ResolutionScope::new(&self.sema);
        let base_index = self.rules.len();
        for (i, parsed) in parsed_rules.iter().enumerate() {
            let resolved = scope.resolve_rule(parsed, base_index + i);
            self.rules.push(resolved);
        }
        Ok(())
    }

    /// Find all matches in a syntax tree.
    pub fn matches(&self) -> SsrMatches {
        // In RA, this iterates over all files in the workspace.
        // Sail equivalent: caller provides the tree via matches_in_tree().
        SsrMatches::default()
    }

    /// Find matches in a specific syntax tree.
    pub fn matches_in_tree(&self, root: &syntax::SyntaxNode, file_id: FileId) -> SsrMatches {
        let mut all = SsrMatches::default();
        for rule in &self.rules {
            let rule_matches = search::search_in_tree(rule, root, file_id);
            all.matches.extend(rule_matches.matches);
        }
        nester::deduplicate(all)
    }

    /// Compute text edits for all matches.
    /// In RA this iterates over all workspace files. In Sail, callers
    /// provide the syntax tree via `edits_for_tree()`.
    pub fn edits(&self) -> FxHashMap<FileId, ide_db::text_edit::TextEdit> {
        // Workspace-wide iteration not yet available; returns empty.
        // Use `edits_for_tree()` for per-file edits.
        FxHashMap::default()
    }

    /// Compute text edits for all matches in a specific syntax tree.
    pub fn edits_for_tree(
        &self,
        root: &syntax::SyntaxNode,
        file_id: FileId,
    ) -> FxHashMap<FileId, ide_db::text_edit::TextEdit> {
        let matches = self.matches_in_tree(root, file_id);
        let raw_edits = replacing::compute_edits_from_matches(&self.rules, &matches);
        let mut result: FxHashMap<FileId, ide_db::text_edit::TextEdit> = FxHashMap::default();
        for (file_range, new_text) in raw_edits {
            result.entry(file_range.file_id).or_insert_with(|| ide_db::text_edit::TextEdit {
                range: file_range.range,
                new_text: new_text.clone(),
            });
        }
        result
    }

    /// Debug why a specific node doesn't match.
    ///
    /// Returns debug info about why matching failed at nodes whose
    /// text equals `snippet`.
    pub fn debug_where_text_equal(
        &self,
        root: &syntax::SyntaxNode,
        file_id: FileId,
        snippet: &str,
    ) -> Vec<MatchDebugInfo> {
        let mut results = Vec::new();
        for node in root.descendants() {
            if node.text().to_string().trim() != snippet.trim() {
                continue;
            }
            for rule in &self.rules {
                let pattern = &rule.pattern.node;
                let matcher = matching::Matcher::new(
                    rule.pattern.placeholders_by_stand_in.clone(),
                    rule.index,
                );
                let matched = match matcher.try_match(pattern, &node) {
                    Ok(captures) => Ok(matching::Match {
                        range: hir_def::in_file::FileRange { file_id, range: node.text_range() },
                        matched_node: node.clone(),
                        placeholder_values: captures,
                        ignored_comments: Vec::new(),
                        rendered_template_paths: FxHashMap::default(),
                        rule_index: rule.index,
                        depth: 0,
                    }),
                    Err(e) => Err(e.reason.unwrap_or_else(|| "unknown".to_string())),
                };
                results.push(MatchDebugInfo { matched });
            }
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rule_from_str() {
        let rule: SsrRule = "foo($a) ==>> bar($a)".parse().unwrap();
        assert!(!rule.raw.is_empty());
    }

    #[test]
    fn parse_rule_missing_separator() {
        let result: Result<SsrRule, _> = "no separator here".parse();
        assert!(result.is_err());
    }

    #[test]
    fn parse_pattern_from_str() {
        let pat: SsrPattern = "foo($a)".parse().unwrap();
        assert!(!pat.raw.is_empty());
    }

    #[test]
    fn parse_pattern_rejects_separator() {
        let result: Result<SsrPattern, _> = "foo($a) ==>> bar($a)".parse();
        assert!(result.is_err());
    }
}
