use tower_lsp::lsp_types::{
    Diagnostic as LspDiagnostic, Position, Range, TextDocumentContentChangeEvent,
};

use super::TextDocument;
use crate::diagnostics::{compute_semantic_diagnostics, Diagnostic, DiagnosticCode, Severity};
use crate::symbols::add_parsed_definitions;
use chumsky::Parser;
use std::{cmp::Ordering, collections::HashMap};

fn best_parsed(
    tokens: Option<&[(sail_parser::Token, sail_parser::Span)]>,
    ast: Option<&sail_parser::SourceFile>,
) -> Option<sail_parser::ParsedFile> {
    match (tokens, ast) {
        (_, Some(ast)) => Some(sail_parser::ParsedFile::from_ast(ast)),
        (Some(tokens), None) => Some(sail_parser::parse_tokens(tokens)),
        (None, None) => None,
    }
}

pub struct File {
    // The source code.
    pub source: TextDocument,

    // The parse result if any. If there isn't one then that is because
    // of a parse error.
    pub tokens: Option<Vec<(sail_parser::Token, sail_parser::Span)>>,

    // Minimal AST for top-level declarations and callable heads.
    pub ast: Option<sail_parser::SourceFile>,

    // Cached semantic index derived from the best available parse.
    pub parsed: Option<sail_parser::ParsedFile>,

    // Go-to definition locations extracted from the file.
    pub definitions: HashMap<String, usize>,

    // Internal diagnostics.
    pub diagnostics: Vec<Diagnostic>,
}

impl File {
    pub fn new(source: String) -> Self {
        let mut f = Self {
            source: TextDocument::new(source),
            tokens: None,
            ast: None,
            parsed: None,
            definitions: HashMap::new(),
            diagnostics: Vec::new(),
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
        self.tokens = result.output().cloned();
        self.ast = self
            .tokens
            .as_ref()
            .and_then(|tokens| sail_parser::parse_source(tokens).into_output());
        self.parsed = best_parsed(self.tokens.as_deref(), self.ast.as_ref());

        let mut definitions = HashMap::with_capacity(self.definitions.len());
        let mut diagnostics = Vec::new();

        if let Some(parsed) = &self.parsed {
            add_parsed_definitions(parsed, &mut definitions);
        } else {
            diagnostics.push(Diagnostic::new(
                DiagnosticCode::ParseError,
                "Error lexing file".to_string(),
                Range::new(Position::new(0, 0), Position::new(0, 0)),
                Severity::Error,
            ));
        }
        for error in result.errors().into_iter() {
            let span = error.span();
            let start = self.source.position_at(span.start);
            let end = self.source.position_at(span.end);
            diagnostics.push(Diagnostic::new(
                DiagnosticCode::ParseError,
                error.to_string(),
                Range::new(start, end),
                Severity::Error,
            ));
        }

        self.definitions = definitions;
        self.diagnostics = diagnostics;

        // RA-style: Add semantic diagnostics
        let semantic = compute_semantic_diagnostics(self);
        self.diagnostics.extend(semantic);
    }

    pub fn parsed(&self) -> Option<&sail_parser::ParsedFile> {
        self.parsed.as_ref()
    }

    pub fn ast(&self) -> Option<&sail_parser::SourceFile> {
        self.ast.as_ref()
    }

    pub fn lsp_diagnostics(&self) -> Vec<LspDiagnostic> {
        self.diagnostics.iter().map(|d| d.to_proto()).collect()
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
        let ast = sail_parser::parse_source(&tokens).into_result().unwrap();

        let parsed = best_parsed(Some(&tokens), Some(&ast)).expect("parsed");
        assert_eq!(parsed, sail_parser::ParsedFile::from_ast(&ast));
    }

    #[test]
    fn falls_back_to_token_index_without_ast() {
        let source = "function foo(x) = x\n";
        let tokens = sail_parser::lexer().parse(source).into_result().unwrap();

        let parsed = best_parsed(Some(&tokens), None).expect("parsed");
        assert_eq!(parsed, sail_parser::parse_tokens(&tokens));
    }
}
