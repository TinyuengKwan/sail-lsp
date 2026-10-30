//! Semantic structural search/replace for Sail expressions.
//! A finder resolves one rule at its invocation position; the IDE owns file
//! selection, include-instance checks and validation of the resulting edits.

#[macro_use]
mod errors;
mod fragments;
mod matching;
mod nester;
mod parsing;
mod replacing;
mod resolving;
mod search;

pub use crate::errors::SsrError;
use matching::Match;
use resolving::{ResolutionScope, ResolvedRule};

pub struct MatchFinder<'db> {
    sema: hir::Semantics<'db>,
    db: &'db dyn salsa::Database,
    rule: ResolvedRule,
}

impl<'db> MatchFinder<'db> {
    /// Parse and resolve an expression replacement in the invocation scope.
    pub fn new(
        db: &'db dyn salsa::Database,
        origin: base_db::FileText,
        offset: usize,
        query: &str,
    ) -> Result<Self, SsrError> {
        let sema = hir::Semantics::new(db);
        let parsed = parsing::parse_rule(query)?;
        for node in [&parsed.pattern, &parsed.template] {
            if node.descendants().any(|n| {
                matches!(
                    n.kind(),
                    parser::SyntaxKind::LET_EXPR
                        | parser::SyntaxKind::MATCH_EXPR
                        | parser::SyntaxKind::FOREACH_EXPR
                        | parser::SyntaxKind::VAR_EXPR
                )
            }) {
                bail!("SSR currently supports expressions without introduced bindings; item, type and binding-pattern rules are unsupported");
            }
        }
        let rule = ResolutionScope::new(&sema, db, origin, offset).resolve_rule(&parsed)?;
        Ok(Self { sema, db, rule })
    }

    /// Compute non-overlapping edits in one file. Filter selections before
    /// deduplication so a selected inner match survives an unselected outer one.
    pub fn edits_for_tree_in_ranges(
        &self,
        root: &syntax::SyntaxNode,
        file: base_db::FileText,
        ranges: &[base_db::TextRange],
    ) -> Vec<ide_db::text_edit::TextEdit> {
        let mut matches = search::search_in_tree(self.db, &self.rule, root);
        matches.retain(|m| {
            (ranges.is_empty()
                || ranges.iter().any(|r| r.contains_range(m.matched_node.text_range())))
                && self.bindings_match(file, m)
        });
        let matches = nester::deduplicate(matches);
        let before = root.text().to_string();
        // Deduplication sorts by source position; emit edits from end to start.
        matches
            .iter()
            .rev()
            .filter_map(|m| {
                let range = m.matched_node.text_range();
                let new_text = replacing::render_template(&self.rule, m);
                let original = &before[usize::from(range.start())..usize::from(range.end())];
                comments_preserved(original, &new_text)
                    .then_some(ide_db::text_edit::TextEdit { range, new_text })
            })
            .collect()
    }

    fn bindings_match(&self, file: base_db::FileText, m: &Match) -> bool {
        let rule = &self.rule;
        // Only references in the pattern itself constrain identity. References
        // captured by a placeholder belong to the matched program, not the rule.
        for node in
            m.matched_node.descendants().filter(|n| n.kind() == parser::SyntaxKind::IDENT_EXPR)
        {
            if m.placeholder_values
                .values()
                .any(|capture| capture.text_range().contains_range(node.text_range()))
            {
                continue;
            }
            let name = node.text().to_string();
            let expected = rule.pattern.resolved_paths.get(name.trim()).cloned();
            let actual =
                self.resolve_name(file, usize::from(node.text_range().start()), name.trim());
            if expected.is_none() || expected != actual {
                return false;
            }
        }
        let template = &rule.template;
        for node in
            template.node.descendants().filter(|n| n.kind() == parser::SyntaxKind::IDENT_EXPR)
        {
            let name = node.text().to_string();
            if template.placeholders_by_stand_in.contains_key(name.trim()) {
                continue;
            }
            let expected = template.resolved_paths.get(name.trim()).cloned();
            let actual = self.resolve_name(
                file,
                usize::from(m.matched_node.text_range().start()),
                name.trim(),
            );
            if expected.is_none() || expected != actual {
                return false;
            }
        }
        true
    }

    fn resolve_name(
        &self,
        file: base_db::FileText,
        offset: usize,
        name: &str,
    ) -> Option<hir::PathResolution> {
        self.sema.resolve_path(self.db, file, offset, name).or_else(|| {
            self.sema.resolve_callable_at(file, offset, name).map(hir::PathResolution::Def)
        })
    }
}

fn comments_preserved(before: &str, after: &str) -> bool {
    fn comments(text: &str) -> Vec<String> {
        let (tree, _) = syntax::parse_text(&format!("let x = {text}"));
        tree.descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| {
                matches!(
                    t.kind(),
                    parser::SyntaxKind::LINE_COMMENT
                        | parser::SyntaxKind::BLOCK_COMMENT
                        | parser::SyntaxKind::DOC_COMMENT
                )
            })
            .map(|t| t.text().to_owned())
            .collect()
    }
    let mut after = comments(after);
    comments(before).into_iter().all(|comment| {
        if let Some(index) = after.iter().position(|c| c == &comment) {
            after.remove(index);
            true
        } else {
            false
        }
    })
}
