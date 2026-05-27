//! Builds upon [`crate::ast::make`] constructors to create ast fragments with
//! optional syntax mappings.
//!
//! Instead of forcing make constructors to perform syntax mapping, we instead
//! let [`SyntaxFactory`] handle constructing the mappings. Care must be taken
//! to remember to feed the syntax mappings into a [`SyntaxEditor`](crate::syntax_editor::SyntaxEditor),
//! if applicable.

mod constructors;

use std::cell::{RefCell, RefMut};

use parser::SyntaxKind;

use crate::syntax_editor::SyntaxMapping;
use crate::syntax_node::SyntaxToken;

/// A factory for creating AST nodes with optional syntax mapping tracking.
pub struct SyntaxFactory {
    // Stored in a RefCell so that factory methods can be &self.
    mappings: Option<RefCell<SyntaxMapping>>,
}

impl SyntaxFactory {
    /// Creates a new [`SyntaxFactory`], generating mappings between input
    /// nodes and generated nodes.
    pub fn with_mappings() -> Self {
        Self { mappings: Some(RefCell::new(SyntaxMapping::default())) }
    }

    /// Creates a [`SyntaxFactory`] without generating mappings.
    pub fn without_mappings() -> Self {
        Self { mappings: None }
    }

    /// Gets all of the tracked syntax mappings, if any.
    pub fn finish_with_mappings(self) -> SyntaxMapping {
        self.mappings.unwrap_or_default().into_inner()
    }

    /// Take all of the tracked syntax mappings, leaving default in its place.
    pub fn take(&self) -> SyntaxMapping {
        self.mappings.as_ref().map(|m| m.take()).unwrap_or_default()
    }

    pub(crate) fn mappings(&self) -> Option<RefMut<'_, SyntaxMapping>> {
        self.mappings.as_ref().map(|it| it.borrow_mut())
    }

    /// Create a whitespace token with the given text.
    pub fn whitespace(&self, text: &str) -> SyntaxToken {
        use rowan::GreenToken;
        // Create a detached whitespace token via rowan.
        let _green = GreenToken::new(rowan::SyntaxKind(SyntaxKind::WHITESPACE as u16), text);
        // We need a way to create a detached SyntaxToken. Use the builder approach.
        use crate::syntax_node::SyntaxNode;
        use rowan::GreenNodeBuilder;
        let mut builder = GreenNodeBuilder::new();
        builder.start_node(rowan::SyntaxKind(SyntaxKind::SOURCE_FILE as u16));
        builder.token(rowan::SyntaxKind(SyntaxKind::WHITESPACE as u16), text);
        builder.finish_node();
        let root = SyntaxNode::new_root(builder.finish());
        root.first_token().unwrap()
    }

    /// Create a token of the given kind with the given text.
    pub fn token(&self, kind: SyntaxKind, text: &str) -> SyntaxToken {
        use crate::syntax_node::SyntaxNode;
        use rowan::GreenNodeBuilder;
        let mut builder = GreenNodeBuilder::new();
        builder.start_node(rowan::SyntaxKind(SyntaxKind::SOURCE_FILE as u16));
        builder.token(rowan::SyntaxKind(kind as u16), text);
        builder.finish_node();
        let root = SyntaxNode::new_root(builder.finish());
        root.first_token().unwrap()
    }
}

impl Default for SyntaxFactory {
    fn default() -> Self {
        Self::without_mappings()
    }
}
