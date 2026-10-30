//! Resolve fixed rule identifiers once in the invocation scope.
use crate::{
    errors::SsrError,
    parsing::{ParsedRule, Placeholder},
};
use rustc_hash::FxHashMap;
use syntax::SyntaxNode;

#[derive(Debug, Clone)]
pub(crate) struct ResolvedRule {
    pub(crate) pattern: ResolvedPattern,
    pub(crate) template: ResolvedPattern,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedPattern {
    pub(crate) placeholders_by_stand_in: FxHashMap<String, Placeholder>,
    pub(crate) node: SyntaxNode,
    pub(crate) resolved_paths: FxHashMap<String, hir::PathResolution>,
}

pub(crate) struct ResolutionScope<'a, 'db> {
    sema: &'a hir::Semantics<'db>,
    db: &'db dyn salsa::Database,
    file: base_db::FileText,
    offset: usize,
}

impl<'a, 'db> ResolutionScope<'a, 'db> {
    pub(crate) fn new(
        sema: &'a hir::Semantics<'db>,
        db: &'db dyn salsa::Database,
        file: base_db::FileText,
        offset: usize,
    ) -> Self {
        Self { sema, db, file, offset }
    }

    pub(crate) fn resolve_rule(&self, parsed: &ParsedRule) -> Result<ResolvedRule, SsrError> {
        let pattern = self.resolve_pattern(&parsed.pattern, &parsed.placeholders_by_stand_in)?;
        let template = self.resolve_pattern(&parsed.template, &parsed.placeholders_by_stand_in)?;
        Ok(ResolvedRule { pattern, template })
    }

    fn resolve_pattern(
        &self,
        node: &SyntaxNode,
        placeholders: &FxHashMap<String, Placeholder>,
    ) -> Result<ResolvedPattern, SsrError> {
        let mut resolved_paths = FxHashMap::default();
        for reference in node.descendants().filter(|n| n.kind() == parser::SyntaxKind::IDENT_EXPR) {
            let name = reference.text().to_string();
            let name = name.trim();
            if placeholders.contains_key(name) {
                continue;
            }
            let Some(resolution) =
                self.sema.resolve_path(self.db, self.file, self.offset, name).or_else(|| {
                    self.sema
                        .resolve_callable_at(self.file, self.offset, name)
                        .map(hir::PathResolution::Def)
                })
            else {
                bail!("Cannot resolve `{}` in the SSR invocation scope", name);
            };
            resolved_paths.insert(name.to_owned(), resolution);
        }
        Ok(ResolvedPattern {
            placeholders_by_stand_in: placeholders.clone(),
            node: node.clone(),
            resolved_paths,
        })
    }
}
