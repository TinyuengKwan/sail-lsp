//! Common AST traits for nodes with names, visibility, etc.
//! These traits provide uniform access to common properties across
//! different AST node types (e.g., "anything with a name").

use super::{support, AstNode};
use crate::syntax_node::SyntaxToken;
use parser::SyntaxKind as SK;

/// AST nodes that have a name identifier.
///
/// `Option<ast::Name>` as in RA. RA wraps names in a typed `Name` node because
/// Rust names can have complex structure (`Self`, `_`, etc.). Sail names are
/// always plain IDENT tokens, so the typed wrapper adds no value here.
pub trait HasName: AstNode {
    fn name(&self) -> Option<SyntaxToken> {
        self.syntax()
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .find(|t| t.kind() == SK::IDENT)
    }

    fn name_text(&self) -> Option<String> {
        self.name().map(|t| t.text().to_string())
    }
}

/// AST nodes that can be marked with visibility attributes.
/// In Sail, visibility is controlled by `@private` attribute
/// rather than `pub`/`pub(crate)` keywords. The parser emits a
/// `Visibility` child node when `private` appears.
pub trait HasVisibility: AstNode {
    fn visibility(&self) -> Option<super::Visibility> {
        support::child(self.syntax())
    }

    fn is_private(&self) -> bool {
        // Primary: check for a Visibility child node (generated accessor)
        if self.visibility().is_some() {
            return true;
        }
        // Fallback: check for `@private` pragma in preceding tokens
        self.syntax()
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|t| t.kind() == SK::KW_PRIVATE || t.text() == "@private")
    }
}

/// AST nodes that can have doc comments (`///`).
pub trait HasDocComments: AstNode {
    fn doc_comments(&self) -> Vec<String> {
        let mut docs = Vec::new();
        // Walk backwards from this node's start to collect preceding doc comments
        if let Some(prev) = self.syntax().prev_sibling_or_token() {
            let mut current = Some(prev);
            while let Some(el) = current {
                match el {
                    rowan::NodeOrToken::Token(ref tok) => {
                        if tok.kind() == SK::DOC_COMMENT {
                            let text = tok.text();
                            // Strip `///` prefix
                            let stripped = text.strip_prefix("///").unwrap_or(text).trim();
                            docs.push(stripped.to_string());
                        } else if tok.kind() == SK::WHITESPACE {
                            // Skip whitespace between doc comments
                        } else {
                            break;
                        }
                    }
                    _ => break,
                }
                current = el.prev_sibling_or_token();
            }
        }
        docs.reverse();
        docs
    }

    fn doc_comment_text(&self) -> Option<String> {
        let docs = self.doc_comments();
        if docs.is_empty() {
            None
        } else {
            Some(docs.join("\n"))
        }
    }
}

/// AST nodes that can have attributes.
/// In Sail, attributes are `$[attr]` pragmas attached to definitions.
pub trait HasAttrs: AstNode {
    fn attrs(&self) -> impl Iterator<Item = super::Attribute> + '_ {
        self.syntax().children().filter_map(super::Attribute::cast)
    }
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use crate::ast::traits::HasName;
    use crate::ast::{AstNode, SourceFile};
    use crate::parsing::parse_text;

    // HasName for CallableDef is auto-generated in nodes.rs via codegen

    #[test]
    fn has_name_extracts_ident() {
        let (root, _) = parse_text("function foo(x) = x + 1\n");
        let sf = SourceFile::cast(root).unwrap();
        let defs = sf.callable_defs();
        assert!(!defs.is_empty());
        // name_text() comes from the generated HasName impl
        assert_eq!(defs[0].name_text(), Some("foo".to_string()));
    }
}
