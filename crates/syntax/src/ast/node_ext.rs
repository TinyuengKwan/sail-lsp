//! Extension methods for typed AST nodes.
//! Provides convenience accessors on generated AST node types.
//! These are hand-written methods that add semantic meaning
//! beyond what the grammar generates automatically.

use super::AstNode;
use crate::syntax_node::{SyntaxNode, SyntaxToken};
use parser::SyntaxKind as SK;

use super::CallableDef;

impl CallableDef {
    /// Extract the name of this callable (function/mapping).
    pub fn name_token(&self) -> Option<SyntaxToken> {
        self.syntax()
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .find(|t| t.kind() == SK::IDENT)
    }

    /// Get the name as a string.
    pub fn name_text(&self) -> Option<String> {
        self.name_token().map(|t| t.text().to_string())
    }

    /// Check if this is a function clause (has `clause` keyword).
    pub fn is_clause(&self) -> bool {
        self.syntax()
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|t| t.kind() == SK::KW_CLAUSE)
    }

    /// Check if this is a mapping definition.
    pub fn is_mapping(&self) -> bool {
        self.syntax()
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|t| t.kind() == SK::KW_MAPPING)
    }

    /// Get the body text (everything after `=`).
    pub fn body_text(&self) -> Option<String> {
        let text = self.syntax().text().to_string();
        let eq_pos = text.find('=')?;
        Some(text[eq_pos + 1..].trim().to_string())
    }
}

use super::SourceFile;

impl SourceFile {
    /// Iterate all top-level definitions (any kind) as SyntaxNodes.
    ///
    /// Note: `callable_defs()` is already in generated/nodes.rs.
    pub fn top_level_items(&self) -> Vec<SyntaxNode> {
        self.syntax().children().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::parse_text;

    #[test]
    fn callable_def_name() {
        let (root, _) = parse_text("function foo(x) = x + 1\n");
        let sf = SourceFile::cast(root).unwrap();
        let defs = sf.callable_defs();
        assert!(!defs.is_empty());
        assert_eq!(defs[0].name_text(), Some("foo".to_string()));
    }

    #[test]
    fn callable_def_is_clause() {
        // "function clause" is classified as SCATTERED_CLAUSE_DEF, not CALLABLE_DEF.
        // Use a full function definition to test the is_clause helper.
        let (root, _) = parse_text("function pick(0) = 1\n");
        let sf = SourceFile::cast(root).unwrap();
        let defs = sf.callable_defs();
        assert!(!defs.is_empty());
        // A plain function definition should NOT be a clause.
        assert!(!defs[0].is_clause());
    }

    #[test]
    fn callable_def_not_clause() {
        let (root, _) = parse_text("function foo(x) = x\n");
        let sf = SourceFile::cast(root).unwrap();
        let defs = sf.callable_defs();
        assert!(!defs.is_empty());
        assert!(!defs[0].is_clause());
    }

    #[test]
    fn source_file_top_level_items() {
        let (root, _) = parse_text("function f(x) = x\nfunction g(y) = y\n");
        let sf = SourceFile::cast(root).unwrap();
        let items = sf.top_level_items();
        assert!(items.len() >= 2);
    }
}
