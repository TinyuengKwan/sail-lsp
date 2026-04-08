use tower_lsp::lsp_types::{Diagnostic as LspDiagnostic, Position, TextDocumentContentChangeEvent};

use super::TextDocument;
use crate::diagnostics::{compute_parse_diagnostics, compute_semantic_diagnostics, Diagnostic};
use crate::symbols::{add_parsed_definitions, CallableSignature};
use chumsky::Parser;
use std::{cmp::Ordering, collections::HashMap, sync::Arc, sync::Mutex};

fn best_parsed(
    core_ast: Option<&sail_parser::core_ast::SourceFile>,
) -> Option<sail_parser::ParsedFile> {
    core_ast.map(sail_parser::ParsedFile::from_core_ast)
}

pub struct File {
    // The source code.
    pub source: TextDocument,

    // The parse result if any. Arc-wrapped for cheap cloning when passing to typecheck thread.
    pub tokens: Option<Arc<Vec<(sail_parser::Token, sail_parser::Span)>>>,

    // Lowered AST used for LSP analysis without depending on the upstream Sail binary.
    pub core_ast: Option<Arc<sail_parser::core_ast::SourceFile>>,

    // Cached semantic index derived from the best available parse.
    pub parsed: Option<sail_parser::ParsedFile>,

    // Cached local type-check result inspired by Sail's type checker pipeline.
    pub type_check: Option<crate::typecheck::TypeCheckResult>,

    // Go-to definition locations extracted from the file.
    pub definitions: HashMap<String, usize>,

    // Cached callable signatures indexed by name for O(1) lookup.
    pub signature_index: HashMap<String, CallableSignature>,

    // Cached per-file reference and implementation counts for code lenses.
    pub ref_counts: HashMap<String, usize>,
    pub impl_counts: HashMap<String, usize>,

    // Parse and semantic diagnostics that are available without type checking.
    base_diagnostics: Vec<Diagnostic>,

    // Cached LSP diagnostics to avoid repeated allocation.
    cached_lsp_diagnostics: Mutex<Option<Vec<LspDiagnostic>>>,

    // Disk-indexed files skip eager type checking to keep workspace scans shallow.
    eager_type_check: bool,
}

impl Clone for File {
    fn clone(&self) -> Self {
        Self {
            source: self.source.clone(),
            tokens: self.tokens.clone(),
            core_ast: self.core_ast.clone(),
            parsed: self.parsed.clone(),
            type_check: self.type_check.clone(),
            definitions: self.definitions.clone(),
            signature_index: self.signature_index.clone(),
            ref_counts: self.ref_counts.clone(),
            impl_counts: self.impl_counts.clone(),
            base_diagnostics: self.base_diagnostics.clone(),
            cached_lsp_diagnostics: Mutex::new(
                self.cached_lsp_diagnostics.lock().unwrap().clone(),
            ),
            eager_type_check: self.eager_type_check,
        }
    }
}

impl File {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new(source: String) -> Self {
        Self::new_with_type_check(source, true)
    }

    pub fn new_lazy(source: String) -> Self {
        Self::new_with_type_check(source, false)
    }

    fn new_with_type_check(source: String, eager_type_check: bool) -> Self {
        let mut f = Self {
            source: TextDocument::new(source),
            tokens: None,
            core_ast: None,
            parsed: None,
            type_check: None,
            definitions: HashMap::new(),
            signature_index: HashMap::new(),
            ref_counts: HashMap::new(),
            impl_counts: HashMap::new(),
            base_diagnostics: Vec::new(),
            cached_lsp_diagnostics: Mutex::new(None),
            eager_type_check,
        };
        f.parse();
        f
    }

    pub fn update(&mut self, changes: Vec<TextDocumentContentChangeEvent>) {
        for change in &changes {
            self.source.update(change);
        }

        self.parse();
    }

    pub fn parse(&mut self) {
        *self.cached_lsp_diagnostics.lock().unwrap() = None;
        let text = self.source.text();
        let result = sail_parser::lexer().parse(text);
        let lex_errors = result.errors().cloned().collect::<Vec<_>>();
        self.tokens = result.output().cloned().map(Arc::new);
        self.core_ast = self
            .tokens
            .as_deref()
            .and_then(|tokens| sail_parser::parse_core_source(tokens).into_output())
            .map(Arc::new);
        self.parsed = best_parsed(self.core_ast.as_deref());
        self.type_check = self
            .eager_type_check
            .then(|| crate::typecheck::check_file(self))
            .flatten();

        let mut definitions = HashMap::with_capacity(self.definitions.len());
        let mut diagnostics = compute_parse_diagnostics(self, &lex_errors);

        if let Some(parsed) = &self.parsed {
            add_parsed_definitions(parsed, &mut definitions);
        }

        self.definitions = definitions;
        self.signature_index = crate::symbols::build_signature_index(self);
        self.build_count_caches();
        diagnostics.extend(compute_semantic_diagnostics(self));
        self.base_diagnostics = diagnostics;
    }

    fn build_count_caches(&mut self) {
        self.ref_counts.clear();
        self.impl_counts.clear();
        let Some(parsed) = &self.parsed else { return };
        for occurrence in &parsed.symbol_occurrences {
            if occurrence.role.is_some()
                || occurrence.scope != Some(sail_parser::Scope::TopLevel)
            {
                continue;
            }
            *self.ref_counts.entry(occurrence.name.clone()).or_insert(0) += 1;
        }
        for decl in &parsed.decls {
            if decl.role != sail_parser::DeclRole::Definition
                || decl.scope != sail_parser::Scope::TopLevel
            {
                continue;
            }
            if matches!(
                decl.kind,
                sail_parser::DeclKind::Function
                    | sail_parser::DeclKind::Mapping
                    | sail_parser::DeclKind::Overload
            ) {
                *self.impl_counts.entry(decl.name.clone()).or_insert(0) += 1;
            }
        }
    }

    pub fn parsed(&self) -> Option<&sail_parser::ParsedFile> {
        self.parsed.as_ref()
    }

    pub fn core_ast(&self) -> Option<&sail_parser::core_ast::SourceFile> {
        self.core_ast.as_deref()
    }

    pub fn type_check(&self) -> Option<&crate::typecheck::TypeCheckResult> {
        self.type_check.as_ref()
    }

    pub fn compute_type_check(&self) -> Option<crate::typecheck::TypeCheckResult> {
        crate::typecheck::check_file(self)
    }

    pub fn set_type_check(&mut self, type_check: Option<crate::typecheck::TypeCheckResult>) {
        *self.cached_lsp_diagnostics.lock().unwrap() = None;
        self.type_check = type_check;
    }

    pub fn lsp_diagnostics(&self) -> Vec<LspDiagnostic> {
        let mut cache = self.cached_lsp_diagnostics.lock().unwrap();
        if let Some(ref cached) = *cache {
            return cached.clone();
        }
        let diagnostics: Vec<LspDiagnostic> = self
            .base_diagnostics
            .iter()
            .chain(
                self.type_check
                    .iter()
                    .flat_map(|type_check| type_check.diagnostics().iter()),
            )
            .map(|d| d.to_proto())
            .collect();
        *cache = Some(diagnostics.clone());
        diagnostics
    }

    fn token_at_offset(
        tokens: &[(sail_parser::Token, sail_parser::Span)],
        offset: usize,
    ) -> Option<&(sail_parser::Token, sail_parser::Span)> {
        let token = tokens.binary_search_by(|(_, span)| {
            if span.start <= offset && offset < span.end {
                Ordering::Equal
            } else if span.start > offset {
                Ordering::Greater
            } else {
                Ordering::Less
            }
        });
        token.ok().map(|i| &tokens[i])
    }

    pub fn token_at(&self, position: Position) -> Option<&(sail_parser::Token, sail_parser::Span)> {
        let offset = self.source.offset_at(&position);
        let tokens = self.tokens.as_deref()?;

        // LSP cursors are often reported at token boundaries; try exact offset first,
        // then the preceding byte to keep identifier-based features stable.
        Self::token_at_offset(tokens, offset).or_else(|| {
            offset
                .checked_sub(1)
                .and_then(|prev| Self::token_at_offset(tokens, prev))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::best_parsed;
    use chumsky::Parser;

    #[test]
    fn builds_parsed_from_core_ast() {
        let source = "val f : bits('n) -> bits('n)\nfunction f(x) = x\n";
        let tokens = sail_parser::lexer().parse(source).into_result().unwrap();
        let core_ast = sail_parser::parse_core_source(&tokens)
            .into_result()
            .unwrap();

        let parsed = best_parsed(Some(&core_ast)).expect("parsed");
        assert_eq!(parsed, sail_parser::ParsedFile::from_core_ast(&core_ast));
    }

    #[test]
    fn returns_none_without_core_ast() {
        assert!(best_parsed(None).is_none());
    }

    #[test]
    fn lazy_files_skip_eager_type_check() {
        let source = "function id(x) = x\n";
        let file = super::File::new_lazy(source.to_string());

        assert!(file.parsed().is_some());
        assert!(file.type_check().is_none());
    }

    #[test]
    fn lazy_type_check_results_extend_diagnostics() {
        let source = "function f(x : bits(32)) -> int = x\n";
        let mut file = super::File::new_lazy(source.to_string());

        assert!(file.lsp_diagnostics().iter().all(|diagnostic| diagnostic
            .code
            .as_ref()
            .map(|code| format!("{code:?}"))
            != Some("String(\"type-error\")".to_string())));

        let result = file.clone().compute_type_check();
        file.set_type_check(result);

        assert!(file.lsp_diagnostics().iter().any(|diagnostic| diagnostic
            .code
            .as_ref()
            .map(|code| format!("{code:?}"))
            == Some("String(\"type-error\")".to_string())));
    }
}
