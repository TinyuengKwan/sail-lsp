//! Resolve fixed rule identifiers once in the invocation scope.
use crate::{
    errors::SsrError,
    parsing::{ParsedRule, Placeholder},
};
use rustc_hash::FxHashMap;
use syntax::SyntaxNode;

#[derive(Debug, Clone)]
pub(crate) struct ResolvedRule {
    pub(crate) kind: crate::fragments::FragmentKind,
    pub(crate) pattern: ResolvedPattern,
    pub(crate) declaration: Option<hir::PathResolution>,
    pub(crate) template: ResolvedPattern,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedPattern {
    pub(crate) placeholders_by_stand_in: FxHashMap<String, Placeholder>,
    pub(crate) node: SyntaxNode,
    pub(crate) bindings: crate::bindings::Bindings,
    pub(crate) resolved_paths: FxHashMap<(String, bool), hir::PathResolution>,
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
        let declaration = if parsed.kind == crate::fragments::FragmentKind::Item {
            syntax::definition_name(&parsed.pattern)
                .filter(|name| !parsed.placeholders_by_stand_in.contains_key(name))
                .map(|name| {
                    let own = hir::source_order::at_offset(self.db, self.file, self.offset)
                        .and_then(|at| {
                            let current = hir::source_order::node(self.db, at)?;
                            (syntax::definition_name(&current).as_deref() == Some(name.as_str()))
                                .then(|| hir::def_at_declaration(self.db, at, &name))
                                .flatten()
                                .map(hir::PathResolution::Def)
                        });
                    use parser::SyntaxKind as SK;
                    let is_type = parsed.pattern.children_with_tokens().any(|e| {
                        matches!(
                            e.kind(),
                            SK::KW_TYPE
                                | SK::KW_STRUCT
                                | SK::KW_UNION
                                | SK::KW_ENUM
                                | SK::KW_BITFIELD
                                | SK::KW_NEWTYPE
                        )
                    });
                    own.or_else(|| {
                        if is_type {
                            self.sema.resolve_type_path(self.file, self.offset, &name)
                        } else {
                            self.sema.resolve_path(self.db, self.file, self.offset, &name).or_else(
                                || {
                                    self.sema
                                        .resolve_callable_at(self.file, self.offset, &name)
                                        .map(hir::PathResolution::Def)
                                },
                            )
                        }
                    })
                    .ok_or_else(|| {
                        SsrError(format!(
                            "Cannot resolve declaration `{name}` in the SSR invocation scope"
                        ))
                    })
                })
                .transpose()?
        } else {
            None
        };
        Ok(ResolvedRule { kind: parsed.kind, pattern, template, declaration })
    }

    fn resolve_pattern(
        &self,
        node: &SyntaxNode,
        placeholders: &FxHashMap<String, Placeholder>,
    ) -> Result<ResolvedPattern, SsrError> {
        let bindings = crate::bindings::Bindings::of_with_globals(node, |name| {
            matches!(
                self.sema.resolve_path(self.db, self.file, self.offset, name.as_str()),
                Some(hir::PathResolution::Def(_))
            )
        });
        let mut resolved_paths = FxHashMap::default();
        for (reference, ty) in crate::bindings::references(node) {
            let name = reference.text();
            if placeholders.contains_key(name)
                || bindings
                    .references
                    .get(&usize::from(reference.text_range().start()))
                    .is_some_and(|v| v.is_some())
            {
                continue;
            }
            let resolution = if ty {
                self.sema.resolve_type_path(self.file, self.offset, name)
            } else {
                self.sema.resolve_path(self.db, self.file, self.offset, name).or_else(|| {
                    self.sema
                        .resolve_callable_at(self.file, self.offset, name)
                        .map(hir::PathResolution::Def)
                })
            };
            let Some(resolution) = resolution else {
                bail!("Cannot resolve `{}` in the SSR invocation scope", name);
            };
            resolved_paths.insert((name.to_owned(), ty), resolution);
        }
        Ok(ResolvedPattern {
            placeholders_by_stand_in: placeholders.clone(),
            node: node.clone(),
            bindings,
            resolved_paths,
        })
    }
}
