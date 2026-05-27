//! Semantic token caching for delta requests.
//! RA caches the last computed `SemanticTokens` per-file in
//! `Arc<Mutex<FxHashMap<Url, SemanticTokens>>>`. When the client sends
//! `textDocument/semanticTokens/full/delta` with a `previousResultId`,
//! we compare the cached tokens with the freshly computed ones and
//! return only the diff (edits).

use std::sync::{Arc, Mutex};

use lsp_types::{SemanticToken, SemanticTokens, SemanticTokensEdit};
use rustc_hash::FxHashMap;

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

    /// Invalidate cache for a file (called when file changes).
    #[allow(dead_code)] // WIP: will be called on didChange notifications
    pub(crate) fn invalidate(&self, uri: &lsp_types::Url) {
        let mut map = self.inner.lock().unwrap();
        map.remove(uri);
    }

    /// Invalidate all cached tokens (called on workspace reload).
    #[allow(dead_code)]
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
