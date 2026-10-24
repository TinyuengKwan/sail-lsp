//! Semantic token types.
//!
//! rust-analyzer keeps a feature's result type in the module that produces
//! it — `HoverResult` in `ide/src/hover.rs`, `InlayHint` in
//! `ide/src/inlay_hints.rs`, `HlRange` in `ide/src/syntax_highlighting.rs`.
//! These lived one crate below, in `ide-db/src/ide_types.rs`, together with
//! forty other types: a crate that cannot see `ide` cannot name what `ide`
//! knows, which is why `HoverFilePosition` exists beside `ide::FilePosition`
//! and why `CompletionItem` was defined twice.

/// A single semantic token (delta-encoded, framework-independent).
///
/// Replaces `lsp_types::SemanticToken`. Same fields — the LSP
/// protocol defines the encoding, not the framework.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HlRange {
    pub delta_line: u32,
    pub delta_start: u32,
    pub length: u32,
    pub token_type: u32,
    pub token_modifiers_bitset: u32,
}

/// A batch of semantic tokens.
///
/// Replaces `lsp_types::SemanticTokens`.
///
/// It used to carry a `result_id` and be accompanied by `HlRangesDelta`
/// and `HlRangesEdit`, for a `compute_semantic_tokens_delta` that nothing
/// called. A delta is between what *this client* was last sent and what it
/// is being sent now — the server is what knows that, and
/// `lsp/semantic_tokens.rs::SemanticTokensCache` is where it is computed,
/// which is where rust-analyzer computes it too (`diff_tokens`). The
/// result id it hands the client is its own.
#[derive(Clone, Debug)]
pub struct HlRanges {
    pub data: Vec<HlRange>,
}

impl HlRanges {
    /// Union active source views before re-encoding LSP deltas. A token
    /// present in multiple views is emitted once, using the first active
    /// view's classification, just like read-only position requests.
    pub(crate) fn merge(views: impl IntoIterator<Item = Self>) -> Self {
        let mut tokens = std::collections::BTreeMap::new();
        for view in views {
            let (mut line, mut col) = (0, 0);
            for token in view.data {
                line += token.delta_line;
                col =
                    if token.delta_line == 0 { col + token.delta_start } else { token.delta_start };
                tokens.entry((line, col)).or_insert(token);
            }
        }
        let (mut previous_line, mut previous_col) = (0, 0);
        let data = tokens
            .into_iter()
            .map(|((line, col), mut token)| {
                token.delta_line = line - previous_line;
                token.delta_start = if line == previous_line { col - previous_col } else { col };
                (previous_line, previous_col) = (line, col);
                token
            })
            .collect();
        Self { data }
    }
}
