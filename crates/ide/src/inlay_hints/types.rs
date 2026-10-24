//! Inlay hint types.
//!
//! rust-analyzer keeps a feature's result type in the module that produces
//! it — `HoverResult` in `ide/src/hover.rs`, `InlayHint` in
//! `ide/src/inlay_hints.rs`, `HlRange` in `ide/src/syntax_highlighting.rs`.
//! These lived one crate below, in `ide-db/src/ide_types.rs`, together with
//! forty other types: a crate that cannot see `ide` cannot name what `ide`
//! knows, which is why `HoverFilePosition` exists beside `ide::FilePosition`
//! and why `CompletionItem` was defined twice.

/// An inlay hint at a byte offset.
///
/// Replaces `lsp_types::InlayHint`. Uses byte offset, not Position.
#[derive(Clone, Debug)]
pub struct InlayHint {
    pub offset: usize,
    pub label: String,
    pub kind: InlayHintKind,
    pub tooltip: Option<String>,
    pub padding_left: Option<bool>,
    pub padding_right: Option<bool>,
    pub data: Option<serde_json::Value>,
}

/// Inlay hint kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InlayHintKind {
    Type,
    Parameter,
    Other,
}
