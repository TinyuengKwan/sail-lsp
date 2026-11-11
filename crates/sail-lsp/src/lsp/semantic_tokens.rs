//! Semantic tokens: the legend, the wire encoding, and the per-file cache
//! that delta requests diff against.
//!
//! rust-analyzer's `lsp/semantic_tokens.rs`. The legend is declared here
//! and nowhere else: `ide` says what a range *is* (`HlTag`, `HlMod`) and
//! this module says which index that is on the wire, and which of those
//! words are LSP's own and which are this server's.
//!
//! The cache: RA keeps the last computed `SemanticTokens` per file. When
//! the client sends `textDocument/semanticTokens/full/delta` with a
//! `previousResultId`, the cached tokens are compared with the freshly
//! computed ones and only the diff (edits) goes back.

use std::sync::{Arc, Mutex};

use lsp_types::{
    Position, Range, SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens,
    SemanticTokensEdit,
};
use rustc_hash::FxHashMap;

macro_rules! define_semantic_token_types {
    (
        standard {
            $($standard:ident),*$(,)?
        }
        custom {
            $(($custom:ident, $string:literal)),*$(,)?
        }

    ) => {
        $(pub(crate) const $standard: SemanticTokenType = SemanticTokenType::$standard;)*
        $(pub(crate) const $custom: SemanticTokenType = SemanticTokenType::new($string);)*

        /// The token type legend, standard types first.
        pub(crate) const SUPPORTED_TYPES: &[SemanticTokenType] = &[
            $(SemanticTokenType::$standard,)*
            $($custom),*
        ];
    };
}

define_semantic_token_types![
    standard {
        KEYWORD,
        FUNCTION,
        TYPE,
        ENUM,
        VARIABLE,
        TYPE_PARAMETER,
        STRING,
        NUMBER,
        OPERATOR,
        STRUCT,
        ENUM_MEMBER,
        PROPERTY,
        COMMENT,
        PARAMETER,
    }

    custom {
        (REGISTER, "register"),
    }
];

macro_rules! define_semantic_token_modifiers {
    (
        standard {
            $($standard:ident),*$(,)?
        }
        custom {
            $(($custom:ident, $string:literal)),*$(,)?
        }

    ) => {

        $(pub(crate) const $standard: SemanticTokenModifier = SemanticTokenModifier::$standard;)*
        $(pub(crate) const $custom: SemanticTokenModifier = SemanticTokenModifier::new($string);)*

        /// The modifier legend, standard modifiers first.
        pub(crate) const SUPPORTED_MODIFIERS: &[SemanticTokenModifier] = &[
            $(SemanticTokenModifier::$standard,)*
            $($custom),*
        ];
    };
}

define_semantic_token_modifiers![
    standard {
        DECLARATION,
        DEFINITION,
        READONLY,
        MODIFICATION,
        DOCUMENTATION,
        STATIC,
    }
    custom {
        (CONTROL_FLOW, "controlFlow"),
    }
];

/// A set of modifiers as the legend's bitset.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModifierSet(pub(crate) u32);

impl std::ops::BitOrAssign<SemanticTokenModifier> for ModifierSet {
    fn bitor_assign(&mut self, rhs: SemanticTokenModifier) {
        let idx = SUPPORTED_MODIFIERS.iter().position(|it| it == &rhs).unwrap();
        self.0 |= 1 << idx;
    }
}

/// The legend index of a token type.
pub(crate) fn type_index(ty: SemanticTokenType) -> u32 {
    SUPPORTED_TYPES.iter().position(|it| *it == ty).unwrap() as u32
}

/// Delta-encodes tokens, as the protocol wants them.
///
/// rust-analyzer's `SemanticTokensBuilder`. Tokens are pushed in position
/// order; a token must not cross a line.
#[derive(Default)]
pub(crate) struct SemanticTokensBuilder {
    prev_line: u32,
    prev_char: u32,
    data: Vec<SemanticToken>,
}

impl SemanticTokensBuilder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Push a new token onto the builder.
    pub(crate) fn push(&mut self, range: Range, token_index: u32, modifier_bitset: u32) {
        let mut push_line = range.start.line;
        let mut push_char = range.start.character;

        if !self.data.is_empty() {
            push_line -= self.prev_line;
            if push_line == 0 {
                push_char -= self.prev_char;
            }
        }

        // A token cannot be multiline
        let token_len = range.end.character - range.start.character;

        let token = SemanticToken {
            delta_line: push_line,
            delta_start: push_char,
            length: token_len,
            token_type: token_index,
            token_modifiers_bitset: modifier_bitset,
        };

        self.data.push(token);

        self.prev_line = range.start.line;
        self.prev_char = range.start.character;
    }

    pub(crate) fn build(self) -> Vec<SemanticToken> {
        self.data
    }
}

/// `line:col` to `line:col + length`, for the builder.
pub(crate) fn token_range(line: u32, col: u32, length: u32) -> Range {
    Range::new(Position::new(line, col), Position::new(line, col + length))
}

/// Per-file semantic token cache.
///
/// in `GlobalStateSnapshot`.
///
/// Thread-safe: accessed from both main thread (invalidation) and
/// handler threads (read + update).
#[derive(Debug, Clone, Default)]
pub(crate) struct SemanticTokensCache {
    inner: Arc<Mutex<FxHashMap<lsp_types::Url, CachedTokens>>>,
}

/// Cached tokens for a single file.
#[derive(Debug, Clone)]
struct CachedTokens {
    /// The result_id returned to the client.
    result_id: String,
    /// The flat token array (data field of SemanticTokens).
    tokens: Vec<SemanticToken>,
}

impl SemanticTokensCache {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Cache the tokens for a file after a full request.
    /// Returns the `SemanticTokens` with a stable `result_id`.
    ///
    /// Called from `handle_semantic_tokens_full`.
    pub(crate) fn cache_full(
        &self,
        uri: &lsp_types::Url,
        tokens: Vec<SemanticToken>,
    ) -> SemanticTokens {
        let result_id = self.next_result_id(uri);
        let mut map = self.inner.lock().unwrap();
        map.insert(
            uri.clone(),
            CachedTokens { result_id: result_id.clone(), tokens: tokens.clone() },
        );
        SemanticTokens { result_id: Some(result_id), data: tokens }
    }

    /// Compute a delta between cached tokens and new tokens.
    ///
    /// Returns `Some(delta)` if the previous_result_id matches and a delta
    /// can be computed. Returns `None` if we must fall back to full tokens.
    ///
    /// Called from `handle_semantic_tokens_full_delta`.
    pub(crate) fn compute_delta(
        &self,
        uri: &lsp_types::Url,
        previous_result_id: &str,
        new_tokens: Vec<SemanticToken>,
    ) -> Option<lsp_types::SemanticTokensDelta> {
        let mut map = self.inner.lock().unwrap();
        let cached = map.get(uri)?;

        if cached.result_id != previous_result_id {
            // Client's result_id is stale — must return full tokens.
            return None;
        }

        let new_result_id = self.next_result_id(uri);
        let edits = diff_tokens(&cached.tokens, &new_tokens);

        // Update the cache with new tokens.
        map.insert(
            uri.clone(),
            CachedTokens { result_id: new_result_id.clone(), tokens: new_tokens },
        );

        Some(lsp_types::SemanticTokensDelta { result_id: Some(new_result_id), edits })
    }

    /// Forget a file's baseline — it is closed.
    pub(crate) fn invalidate(&self, uri: &lsp_types::Url) {
        let mut map = self.inner.lock().unwrap();
        map.remove(uri);
    }

    /// Forget every baseline, because the client has been asked to re-pull.
    pub(crate) fn clear(&self) {
        let mut map = self.inner.lock().unwrap();
        map.clear();
    }

    /// Generate a unique result_id for a file.
    fn next_result_id(&self, uri: &lsp_types::Url) -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("{}#{}", uri.path(), n)
    }
}

/// Compute the minimal set of edits to transform `old` tokens into `new` tokens.
///
/// Uses a simple approach: find the first and last differing position,
/// emit a single edit replacing that range. This matches RA's behavior
/// for most practical cases (single character edits shift all subsequent tokens).
fn diff_tokens(old: &[SemanticToken], new: &[SemanticToken]) -> Vec<SemanticTokensEdit> {
    if old == new {
        return Vec::new();
    }

    // Find first differing index.
    let prefix_len = old.iter().zip(new.iter()).take_while(|(a, b)| a == b).count();

    // Find last differing index (from the end).
    let suffix_len = old[prefix_len..]
        .iter()
        .rev()
        .zip(new[prefix_len..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();

    let old_changed = &old[prefix_len..old.len() - suffix_len];
    let new_changed = &new[prefix_len..new.len() - suffix_len];

    // Each SemanticToken is 5 u32s in the wire format.
    let start = (prefix_len * 5) as u32;
    let delete_count = (old_changed.len() * 5) as u32;
    let data: Vec<SemanticToken> = new_changed.to_vec();

    vec![SemanticTokensEdit { start, delete_count, data: Some(data) }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok(line: u32, start: u32, len: u32, ty: u32) -> SemanticToken {
        SemanticToken {
            delta_line: line,
            delta_start: start,
            length: len,
            token_type: ty,
            token_modifiers_bitset: 0,
        }
    }

    #[test]
    fn cache_full_returns_result_id() {
        let cache = SemanticTokensCache::new();
        let uri = lsp_types::Url::parse("file:///test.sail").unwrap();
        let tokens = vec![tok(0, 0, 3, 1), tok(0, 4, 2, 2)];
        let result = cache.cache_full(&uri, tokens.clone());
        assert!(result.result_id.is_some());
        assert_eq!(result.data.len(), 2);
    }

    #[test]
    fn delta_with_matching_result_id() {
        let cache = SemanticTokensCache::new();
        let uri = lsp_types::Url::parse("file:///test.sail").unwrap();

        let old_tokens = vec![tok(0, 0, 3, 1), tok(0, 4, 2, 2)];
        let result = cache.cache_full(&uri, old_tokens);
        let result_id = result.result_id.unwrap();

        // Changed: second token is now longer.
        let new_tokens = vec![tok(0, 0, 3, 1), tok(0, 4, 5, 2)];
        let delta = cache.compute_delta(&uri, &result_id, new_tokens);
        assert!(delta.is_some());
        let delta = delta.unwrap();
        assert!(!delta.edits.is_empty());
    }

    #[test]
    fn delta_with_stale_result_id_returns_none() {
        let cache = SemanticTokensCache::new();
        let uri = lsp_types::Url::parse("file:///test.sail").unwrap();

        let tokens = vec![tok(0, 0, 3, 1)];
        cache.cache_full(&uri, tokens);

        // Use a wrong result_id.
        let delta = cache.compute_delta(&uri, "wrong_id", vec![tok(0, 0, 3, 1)]);
        assert!(delta.is_none());
    }

    #[test]
    fn delta_unchanged_returns_empty_edits() {
        let cache = SemanticTokensCache::new();
        let uri = lsp_types::Url::parse("file:///test.sail").unwrap();

        let tokens = vec![tok(0, 0, 3, 1), tok(0, 4, 2, 2)];
        let result = cache.cache_full(&uri, tokens.clone());
        let result_id = result.result_id.unwrap();

        let delta = cache.compute_delta(&uri, &result_id, tokens);
        let delta = delta.unwrap();
        assert!(delta.edits.is_empty());
    }

    #[test]
    fn invalidate_removes_entry() {
        let cache = SemanticTokensCache::new();
        let uri = lsp_types::Url::parse("file:///test.sail").unwrap();

        let tokens = vec![tok(0, 0, 3, 1)];
        let result = cache.cache_full(&uri, tokens);
        let result_id = result.result_id.unwrap();

        cache.invalidate(&uri);
        let delta = cache.compute_delta(&uri, &result_id, vec![]);
        assert!(delta.is_none());
    }
}
