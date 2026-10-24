//! Where the user can jump to.
//!
//! `NavigationTarget` is the result type of every feature that produces a
//! clickable location — goto-definition, goto-declaration, references,
//! call hierarchy, the document outline. rust-analyzer keeps it here, in
//! `ide`, for the reason this workspace already records about `HoverResult`
//! and `InlayHint`: a feature's result type lives with the feature.
//!
//! There were **two** of these. This module held rust-analyzer's shape —
//! `full_range`, an optional `focus_range`, three builder methods — used by
//! nothing but its own re-export, while every feature built a flat one in
//! `ide_db::ide_types` that `ide-db` itself only mentioned in a test. One
//! fact, two types, and the builders had no callers because the type they
//! built was not the type anything returned.
//!
//! The surviving fields are the ones the features fill in. `url` is the
//! Sail extension: the server addresses files by URL at its boundary and
//! `FileId` is what salsa knows, so a target carries both until the last
//! caller stops needing the first.

use base_db::TextRange;
use ide_db::defs::SymbolKind;

/// A "where to go" result for navigation features.
#[derive(Clone, Debug)]
pub struct NavigationTarget {
    /// File containing this target.
    ///
    /// Optional for backward compat — will become required.
    pub file_id: Option<base_db::FileId>,
    /// URL of the file (backward compat, will be derived from file_id).
    pub url: url::Url,
    /// Name of the symbol.
    pub name: String,
    /// Symbol kind.
    pub kind: SymbolKind,
    /// Full range of the entire definition.
    pub full_range: TextRange,
    /// Range of just the name, where the cursor should land.
    pub focus_range: TextRange,
    /// Additional detail (type signature, etc.).
    pub detail: Option<String>,
    /// Documentation string.
    pub docs: Option<String>,
    /// Nested targets (e.g. a struct's fields inside the struct).
    pub children: Vec<NavigationTarget>,
}

impl NavigationTarget {
    /// A target with the two ranges every navigation answer needs.
    pub fn new(
        url: url::Url,
        name: impl Into<String>,
        kind: SymbolKind,
        full_range: TextRange,
        focus_range: TextRange,
    ) -> Self {
        Self {
            file_id: None,
            url,
            name: name.into(),
            kind,
            full_range,
            focus_range,
            detail: None,
            docs: None,
            children: Vec::new(),
        }
    }

    /// Builder: the file salsa knows this target by.
    pub fn with_file_id(mut self, file_id: base_db::FileId) -> Self {
        self.file_id = Some(file_id);
        self
    }

    /// Builder: the signature or type shown beside the name.
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Builder: the documentation comment attached to the definition.
    pub fn with_docs(mut self, docs: impl Into<String>) -> Self {
        self.docs = Some(docs.into());
        self
    }

    // There is no `with_children`. The outline builds a parent and then
    // pushes into `children` as it finds them, which a consuming builder
    // cannot express — and a builder with no caller is what this module
    // was made of.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_target_builder() {
        let nav = NavigationTarget::new(
            url::Url::parse("file:///test.sail").expect("a url"),
            "foo",
            SymbolKind::Function,
            base_db::text_range(0, 20),
            base_db::text_range(9, 12),
        )
        .with_detail("val foo : int -> bool")
        .with_file_id(base_db::FileId::from_raw(0));

        assert_eq!(nav.name, "foo");
        assert_eq!(nav.kind, SymbolKind::Function);
        assert_eq!(nav.detail.as_deref(), Some("val foo : int -> bool"));
        assert_eq!(nav.file_id, Some(base_db::FileId::from_raw(0)));
    }
}
