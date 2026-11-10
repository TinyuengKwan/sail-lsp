//! Semantic structural search/replace for Sail fragments.
//! A finder resolves one rule at its invocation position; the IDE owns file
//! selection, include-instance checks and validation of the resulting edits.

#[macro_use]
mod errors;
mod bindings;
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
    /// Fixed references in both sides, after resolving the invocation scope.
    /// The workspace layer compares physical declaration identities across
    /// include contexts before allowing any physical source replacement.
    pub fn resolved_bindings(&self) -> Vec<(&str, bool, &hir::PathResolution)> {
        let mut bindings: Vec<_> = self
            .rule
            .pattern
            .resolved_paths
            .iter()
            .chain(&self.rule.template.resolved_paths)
            .map(|((name, ty), resolution)| (name.as_str(), *ty, resolution))
            .collect();
        if let Some(declaration) = &self.rule.declaration {
            bindings.push(("<declaration>", false, declaration));
        }
        bindings.sort_by_key(|(name, ty, _)| (*name, *ty));
        bindings
    }

    /// Search uses the same binding identities as an identity replacement.
    pub fn for_search(
        db: &'db dyn salsa::Database,
        origin: base_db::FileText,
        offset: usize,
        pattern: &str,
    ) -> Result<Self, SsrError> {
        let (_, bare) = fragments::FragmentKind::split(pattern);
        Self::new(db, origin, offset, &format!("{pattern}\n ==>> {bare}"))
    }

    /// Read-only matches, including nested occurrences. Replacement overlap
    /// rules do not apply when reporting locations. The workspace caller owns
    /// physical-location deduplication and final ordering across semantic views.
    pub fn ranges_for_tree(
        &self,
        root: &syntax::SyntaxNode,
        file: base_db::FileText,
        ranges: &[base_db::TextRange],
    ) -> Vec<base_db::TextRange> {
        search::search_in_tree(self.db, &self.rule, root)
            .into_iter()
            .filter(|m| {
                (ranges.is_empty()
                    || ranges.iter().any(|r| r.contains_range(m.matched_node.text_range())))
                    && self.bindings_match(file, m)
            })
            .map(|m| m.matched_node.text_range())
            .collect()
    }

    /// Parse and resolve a fragment replacement in the invocation scope.
    pub fn new(
        db: &'db dyn salsa::Database,
        origin: base_db::FileText,
        offset: usize,
        query: &str,
    ) -> Result<Self, SsrError> {
        let sema = hir::Semantics::new(db);
        let parsed = parsing::parse_rule(query)?;
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
                let new_text = replacing::render_template(&self.rule, m, |name| {
                    matches!(
                        self.resolve_name(
                            file,
                            usize::from(m.matched_node.text_range().start()),
                            name.as_str()
                        ),
                        Some(hir::PathResolution::Def(_))
                    )
                })?;
                let original = &before[usize::from(range.start())..usize::from(range.end())];
                comments_preserved(original, &new_text)
                    .then_some(ide_db::text_edit::TextEdit { range, new_text })
            })
            .collect()
    }

    fn bindings_match(&self, file: base_db::FileText, m: &Match) -> bool {
        let rule = &self.rule;
        if let Some(expected) = &rule.declaration {
            let (Some(name), Some(span)) = (
                syntax::definition_name(&m.matched_node),
                syntax::definition_name_span(&m.matched_node),
            ) else {
                return false;
            };
            if self.sema.resolve_path(self.db, file, span.start, &name).as_ref() != Some(expected) {
                return false;
            }
        }
        let actual = bindings::Bindings::of_with_globals(&m.matched_node, |name| {
            matches!(
                self.resolve_name(
                    file,
                    usize::from(m.matched_node.text_range().start()),
                    name.as_str()
                ),
                Some(hir::PathResolution::Def(_))
            )
        });
        for (&reference, target) in &rule.pattern.bindings.references {
            let Some(target) = target else { continue };
            let (Some(actual_ref), Some(actual_target)) =
                (m.token_mapping.get(&reference), m.token_mapping.get(target))
            else {
                return false;
            };
            if actual.references.get(actual_ref) != Some(&Some(*actual_target)) {
                return false;
            }
        }
        for (token, ty) in bindings::references(&m.matched_node) {
            let offset = usize::from(token.text_range().start());
            if m.placeholder_values
                .values()
                .any(|capture| capture.text_range().contains_range(token.text_range()))
                || actual.references.get(&offset).is_some_and(|v| v.is_some())
            {
                continue;
            }
            let name = token.text();
            let expected = rule.pattern.resolved_paths.get(&(name.to_owned(), ty));
            let found = self.resolve_reference(file, offset, name, ty);
            if expected.is_none() || expected != found.as_ref() {
                return false;
            }
        }
        for (token, ty) in bindings::references(&rule.template.node) {
            let name = token.text();
            if rule.template.placeholders_by_stand_in.contains_key(name)
                || rule
                    .template
                    .bindings
                    .references
                    .get(&usize::from(token.text_range().start()))
                    .is_some_and(|v| v.is_some())
            {
                continue;
            }
            let expected = rule.template.resolved_paths.get(&(name.to_owned(), ty));
            // The template's text does not exist at the match yet, so there
            // is no token for `resolve_reference` to see a parameter or `let`
            // through. Ask the scope at the match site directly: a local of
            // that name would capture the rewritten text.
            let offset = usize::from(m.matched_node.text_range().start());
            let local = (!ty)
                .then(|| {
                    self.sema
                        .locals_in_scope(file, offset)
                        .into_iter()
                        .find(|local| local.name.as_str() == name)
                        .map(hir::PathResolution::Local)
                })
                .flatten();
            let found = local.or_else(|| self.resolve_reference(file, offset, name, ty));
            if expected.is_none() || expected != found.as_ref() {
                return false;
            }
        }
        true
    }

    fn resolve_reference(
        &self,
        file: base_db::FileText,
        offset: usize,
        name: &str,
        ty: bool,
    ) -> Option<hir::PathResolution> {
        if ty {
            self.sema.resolve_type_path(file, offset, name)
        } else {
            self.resolve_name(file, offset, name)
        }
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
