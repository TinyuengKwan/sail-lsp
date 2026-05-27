//! Representation of a navigation result (go-to-definition, find references, etc.)
//! `NavigationTarget` is the central type for "jump to definition"-style features.
//! Each IDE feature that produces a clickable location returns one or more of these.

use base_db::FileId;
use ide_db::defs::SymbolKind;

/// A "where to go" result for navigation features.
/// Used by goto-definition, goto-declaration, goto-implementation,
/// find-references, rename, call-hierarchy, etc.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NavigationTarget {
    /// File containing the target.
    pub file_id: FileId,
    /// Full syntactic range of the definition (including body, attrs, docs).
    pub full_range: rowan::TextRange,
    /// Range of the identifier (where the cursor should land).
    /// `None` for synthetic targets.
    pub focus_range: Option<rowan::TextRange>,
    /// Name of the symbol.
    pub name: String,
    /// Kind of symbol (function, struct, enum, etc.).
    pub kind: Option<SymbolKind>,
    /// Name of the containing entity (parent module/struct).
    pub container_name: Option<String>,
    /// Human-readable description (signature, type info).
    pub description: Option<String>,
    /// Documentation text.
    pub docs: Option<String>,
}

impl NavigationTarget {
    /// Create a minimal navigation target.
    pub fn new(file_id: FileId, name: impl Into<String>, full_range: rowan::TextRange) -> Self {
        Self {
            file_id,
            full_range,
            focus_range: None,
            name: name.into(),
            kind: None,
            container_name: None,
            description: None,
            docs: None,
        }
    }

    /// Builder: set focus range.
    pub fn with_focus_range(mut self, range: rowan::TextRange) -> Self {
        self.focus_range = Some(range);
        self
    }

    /// Builder: set symbol kind.
    pub fn with_kind(mut self, kind: SymbolKind) -> Self {
        self.kind = Some(kind);
        self
    }

    /// Builder: set description.
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Builder: set documentation.
    pub fn with_docs(mut self, docs: impl Into<String>) -> Self {
        self.docs = Some(docs.into());
        self
    }

    /// Builder: set container name.
    pub fn with_container_name(mut self, name: impl Into<String>) -> Self {
        self.container_name = Some(name.into());
        self
    }
}

/// Trait for HIR types that can be converted to a `NavigationTarget`.
pub trait ToNav {
    fn to_nav(&self, file_id: FileId) -> NavigationTarget;
}

/// Trait for HIR types that can provide a `NavigationTarget` from database queries.
pub trait TryToNav {
    fn try_to_nav(&self, file_id: FileId) -> Option<NavigationTarget>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_target_builder() {
        let nav = NavigationTarget::new(
            FileId::from_raw(0),
            "foo",
            rowan::TextRange::new(0.into(), 10.into()),
        )
        .with_kind(SymbolKind::Function)
        .with_description("val foo : int -> bool");

        assert_eq!(nav.name, "foo");
        assert_eq!(nav.kind, Some(SymbolKind::Function));
        assert_eq!(nav.description.as_deref(), Some("val foo : int -> bool"));
    }
}
