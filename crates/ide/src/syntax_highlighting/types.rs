//! Semantic token types.
//!
//! rust-analyzer keeps a feature's result type in the module that produces
//! it — `HoverResult` in `ide/src/hover.rs`, `InlayHint` in
//! `ide/src/inlay_hints.rs`, `HlRange` in `ide/src/syntax_highlighting.rs`.
//! These lived one crate below, in `ide-db/src/ide_types.rs`, together with
//! forty other types: a crate that cannot see `ide` cannot name what `ide`
//! knows, which is why `HoverFilePosition` exists beside `ide::FilePosition`
//! and why `CompletionItem` was defined twice.

use super::tags::Highlight;

/// A highlighted range: one line of one token, and what it is.
///
/// rust-analyzer's `HlRange` is a `TextRange` and a `Highlight`; the
/// protocol's delta encoding and legend indices are the server's business
/// (`lsp/to_proto.rs`). This used to be the wire format itself — five
/// `u32`s with the legend baked into two of them — so the server could
/// neither drop a class of token nor substitute a standard type for a
/// custom one without knowing which number meant what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HlRange {
    /// 0-based line.
    pub line: u32,
    /// 0-based UTF-8 byte column.
    pub col: u32,
    /// Length in bytes; never crosses a line.
    pub length: u32,
    pub highlight: Highlight,
}

/// The highlighted ranges of a file, in position order.
///
/// It used to carry a `result_id` and be accompanied by `HlRangesDelta`
/// and `HlRangesEdit`, for a `compute_semantic_tokens_delta` that nothing
/// called. A delta is between what *this client* was last sent and what it
/// is being sent now — the server is what knows that, and
/// `lsp/semantic_tokens.rs::SemanticTokensCache` is where it is computed,
/// which is where rust-analyzer computes it too (`diff_tokens`). The
/// result id it hands the client is its own.
#[derive(Clone, Debug, Default)]
pub struct HlRanges {
    pub data: Vec<HlRange>,
}

impl HlRanges {
    /// Union active source views. A token present in multiple views is
    /// emitted once, using the first active view's classification, just
    /// like read-only position requests.
    pub(crate) fn merge(views: impl IntoIterator<Item = Self>) -> Self {
        let mut tokens = std::collections::BTreeMap::new();
        for view in views {
            for token in view.data {
                tokens.entry((token.line, token.col)).or_insert(token);
            }
        }
        Self { data: tokens.into_values().collect() }
    }
}
