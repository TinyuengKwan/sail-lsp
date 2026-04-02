use tower_lsp::lsp_types::{Diagnostic as LspDiagnostic, Position, TextDocumentContentChangeEvent};

use super::TextDocument;
use crate::diagnostics::{compute_parse_diagnostics, compute_semantic_diagnostics, Diagnostic};
use crate::symbols::add_parsed_definitions;
use chumsky::Parser;
use std::{cmp::Ordering, collections::HashMap};

fn best_parsed(
    tokens: Option<&[(sail_parser::Token, sail_parser::Span)]>,
    core_ast: Option<&sail_parser::core_ast::SourceFile>,
) -> Option<sail_parser::ParsedFile> {
    match (tokens, core_ast) {
        (_, Some(ast)) => Some(sail_parser::ParsedFile::from_core_ast(ast)),
        (Some(tokens), None) => Some(sail_parser::parse_tokens(tokens)),
        (None, None) => None,
    }
}

#[derive(Clone)]
pub struct File {
    // The source code.
    pub source: TextDocument,

    // The parse result if any. If there isn't one then that is because
    // of a parse error.
    pub tokens: Option<Vec<(sail_parser::Token, sail_parser::Span)>>,

    // Lowered AST used for LSP analysis without depending on the upstream Sail binary.
    pub core_ast: Option<sail_parser::core_ast::SourceFile>,

    // Cached semantic index derived from the best available parse.
    pub parsed: Option<sail_parser::ParsedFile>,

    // Cached local type-check result inspired by Sail's type checker pipeline.
    pub type_check: Option<crate::typecheck::TypeCheckResult>,

    // Go-to definition locations extracted from the file.
    pub definitions: HashMap<String, usize>,

    // Parse and semantic diagnostics that are available without type checking.
    base_diagnostics: Vec<Diagnostic>,

    // Disk-indexed files skip eager type checking to keep workspace scans shallow.
    eager_type_check: bool,
}

impl File {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new(source: String) -> Self {
        Self::new_with_type_check(source, true)
    }

    pub fn new_lazy(source: String) -> Self {
        Self::new_with_type_check(source, false)
    }

    pub fn new_indexed(source: String) -> Self {
        Self::new_lazy(source)
    }

    fn new_with_type_check(source: String, eager_type_check: bool) -> Self {
        let mut f = Self {
            source: TextDocument::new(source),
            tokens: None,
            core_ast: None,
            parsed: None,
            type_check: None,
            definitions: HashMap::new(),
            base_diagnostics: Vec::new(),
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
        let text = self.source.text();
        let result = sail_parser::lexer().parse(text);
        let lex_errors = result.errors().cloned().collect::<Vec<_>>();
        self.tokens = result.output().cloned();
        self.core_ast = self
            .tokens
            .as_ref()
            .and_then(|tokens| sail_parser::parse_core_source(tokens).into_output());
        self.parsed = best_parsed(self.tokens.as_deref(), self.core_ast.as_ref());
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
        diagnostics.extend(compute_semantic_diagnostics(self));
        self.base_diagnostics = diagnostics;
    }

    pub fn parsed(&self) -> Option<&sail_parser::ParsedFile> {
        self.parsed.as_ref()
    }

    pub fn core_ast(&self) -> Option<&sail_parser::core_ast::SourceFile> {
        self.core_ast.as_ref()
    }

    pub fn type_check(&self) -> Option<&crate::typecheck::TypeCheckResult> {
        self.type_check.as_ref()
    }

    pub fn compute_type_check(&self) -> Option<crate::typecheck::TypeCheckResult> {
        crate::typecheck::check_file(self)
    }

    pub fn set_type_check(&mut self, type_check: Option<crate::typecheck::TypeCheckResult>) {
        self.type_check = type_check;
    }

    pub fn lsp_diagnostics(&self) -> Vec<LspDiagnostic> {
        self.base_diagnostics
            .iter()
            .chain(
                self.type_check
                    .iter()
                    .flat_map(|type_check| type_check.diagnostics().iter()),
            )
            .map(|d| d.to_proto())
            .collect()
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
        let tokens = self.tokens.as_ref()?;

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
    fn prefers_ast_index_when_available() {
        let source = "val f : bits('n) -> bits('n)\nfunction f(x) = x\n";
        let tokens = sail_parser::lexer().parse(source).into_result().unwrap();
        let core_ast = sail_parser::parse_core_source(&tokens)
            .into_result()
            .unwrap();

        let parsed = best_parsed(Some(&tokens), Some(&core_ast)).expect("parsed");
        assert_eq!(parsed, sail_parser::ParsedFile::from_core_ast(&core_ast));
    }

    #[test]
    fn falls_back_to_token_index_without_ast() {
        let source = "function foo(x) = x\n";
        let tokens = sail_parser::lexer().parse(source).into_result().unwrap();

        let parsed = best_parsed(Some(&tokens), None).expect("parsed");
        assert_eq!(parsed, sail_parser::parse_tokens(&tokens));
    }

    #[test]
    fn indexed_files_skip_eager_type_check() {
        let source = "function id(x) = x\n";
        let file = super::File::new_indexed(source.to_string());

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
