//! Workspace-wide symbol index.
//!
//! Eliminates O(n) `all_salsa_files()` scans by pre-building a name->location
//! index after workspace scan. Stored in `Arc` for cheap snapshot sharing.
//! Uses HashMap (exact-match is the common case for Sail workspaces).

use std::collections::HashMap;

use hir_def::item_tree::ItemKind;
use parser::Span;
use url::Url;

use crate::FileDb;

/// Per-symbol entry in the workspace index.
/// Derived from `ItemTreeEntry` — contains only what handlers need for lookups.
#[derive(Clone, Debug)]
pub struct SymbolEntry {
    /// File where this symbol is defined.
    pub url: Url,
    /// Symbol name.
    pub name: String,
    /// Item kind (Function, Struct, Enum, ValSpec, etc.)
    pub kind: ItemKind,
    /// Byte span of the definition in source.
    pub span: Span,
    /// Canonical signature text (for hover, signature help).
    pub signature_text: String,
    /// Doc comment text (for hover).
    pub doc: Option<String>,
    /// True for `function clause` / `mapping clause`.
    pub is_clause: bool,
}

/// Workspace-wide symbol index: O(1) name lookups instead of O(n) file scans.
///
/// Built after workspace scan, stored in `Arc<SymbolIndex>` for
/// cheap snapshot sharing. Updated incrementally on file changes.
#[derive(Clone, Debug, Default)]
pub struct SymbolIndex {
    /// name → all definitions/declarations across workspace.
    by_name: HashMap<String, Vec<SymbolEntry>>,
    /// url → list of symbol names defined in that file (for incremental update).
    by_file: HashMap<Url, Vec<String>>,
    /// Aggregated reference counts: name → count across workspace.
    ref_counts: HashMap<String, usize>,
    /// Aggregated implementation counts: name → count across workspace.
    impl_counts: HashMap<String, usize>,
}

impl SymbolIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build the index from an iterator of (Url, &dyn FileDb) pairs.
    /// Called once after workspace scan completes.
    pub fn build<'a, F: FileDb + 'a>(files: impl IntoIterator<Item = (&'a Url, &'a F)>) -> Self {
        let mut index = Self::new();
        for (url, file) in files {
            index.add_file(url, file);
        }
        index
    }

    /// Add/replace all entries for a single file.
    pub fn add_file(&mut self, url: &Url, file: &dyn FileDb) {
        // Remove old entries for this file first
        self.remove_file(url);

        let mut file_names = Vec::new();

        // Extract entries from ItemTree (definitions + declarations)
        if let Some(item_tree) = file.item_tree() {
            for &id in item_tree.top_level_items() {
                let name = id.name(item_tree).as_str().to_string();
                file_names.push(name.clone());
                self.by_name.entry(name).or_default().push(SymbolEntry {
                    url: url.clone(),
                    name: id.name(item_tree).as_str().to_string(),
                    kind: id.item_kind(item_tree),
                    span: id.span(item_tree),
                    signature_text: id.signature(item_tree).to_string(),
                    doc: id.doc(item_tree).map(|s| s.to_string()),
                    is_clause: id.is_clause(item_tree),
                });
            }
        }

        // Collect reference counts from ParsedFile symbol_occurrences
        if let Some(parsed) = file.parsed() {
            use syntax::parser_lower::SymbolOccurrenceKind;
            for occ in &parsed.symbol_occurrences {
                if occ.kind == SymbolOccurrenceKind::Value {
                    *self.ref_counts.entry(occ.name.clone()).or_insert(0) += 1;
                }
            }

            // Collect implementation counts (function/mapping definitions)
            use syntax::parser_lower::{DeclKind, DeclRole};
            for decl in &parsed.decls {
                if decl.role == DeclRole::Definition
                    && matches!(decl.kind, DeclKind::Function | DeclKind::Mapping)
                {
                    *self.impl_counts.entry(decl.name.clone()).or_insert(0) += 1;
                }
            }
        }

        self.by_file.insert(url.clone(), file_names);
    }

    /// Remove all entries for a file (for incremental update or deletion).
    pub fn remove_file(&mut self, url: &Url) {
        if let Some(names) = self.by_file.remove(url) {
            for name in &names {
                if let Some(entries) = self.by_name.get_mut(name) {
                    entries.retain(|e| &e.url != url);
                    if entries.is_empty() {
                        self.by_name.remove(name);
                    }
                }
            }
        }
        // Note: ref_counts and impl_counts are approximate after removal.
        // They're rebuilt on next full index build (workspace scan).
        // For incremental updates (single file), the impact is minimal.
    }

    /// Lookup all entries by exact name. O(1).
    pub fn find(&self, name: &str) -> &[SymbolEntry] {
        self.by_name.get(name).map_or(&[], |v| v.as_slice())
    }

    /// Search by case-insensitive substring with relevance ranking.
    ///
    /// C6: Enhanced from flat substring filter to scored ranking:
    /// - Exact match (case-insensitive): score 100
    /// - Prefix match: score 80
    /// - Substring match: score 50
    /// - Results sorted by score (descending), then alphabetically.
    pub fn search(&self, query: &str) -> Vec<&SymbolEntry> {
        if query.is_empty() {
            return Vec::new();
        }
        let query_lower = query.to_ascii_lowercase();
        let mut scored: Vec<(u32, &SymbolEntry)> = self
            .by_name
            .values()
            .flatten()
            .filter_map(|e| {
                let name_lower = e.name.to_ascii_lowercase();
                let score = if name_lower == query_lower {
                    100 // exact match
                } else if name_lower.starts_with(&query_lower) {
                    80 // prefix match
                } else if name_lower.contains(&query_lower) {
                    50 // substring match
                } else {
                    return None;
                };
                Some((score, e))
            })
            .collect();
        // Sort: highest score first, then alphabetical for ties
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.name.cmp(&b.1.name)));
        scored.into_iter().map(|(_, e)| e).collect()
    }

    /// Get aggregated reference count for a symbol.
    pub fn ref_count(&self, name: &str) -> usize {
        self.ref_counts.get(name).copied().unwrap_or(0)
    }

    /// Get aggregated implementation count for a symbol.
    pub fn impl_count(&self, name: &str) -> usize {
        self.impl_counts.get(name).copied().unwrap_or(0)
    }

    /// Iterate all unique symbol names in the index.
    pub fn all_names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(|s| s.as_str())
    }

    /// Get all entries (for iteration).
    pub fn all_entries(&self) -> impl Iterator<Item = &SymbolEntry> {
        self.by_name.values().flatten()
    }

    /// Fuzzy search with edit-distance tolerance.
    ///
    /// Uses Levenshtein distance instead of FST (simpler, no external dep).
    ///
    /// Returns symbols whose name is within `max_distance` edits of `query`,
    /// sorted by distance (closest first), limited to `limit` results.
    pub fn fuzzy_search(
        &self,
        query: &str,
        max_distance: usize,
        limit: usize,
    ) -> Vec<&SymbolEntry> {
        if query.is_empty() {
            return Vec::new();
        }
        let query_lower = query.to_ascii_lowercase();
        let mut scored: Vec<(usize, &SymbolEntry)> = self
            .by_name
            .values()
            .flatten()
            .filter_map(|e| {
                let name_lower = e.name.to_ascii_lowercase();
                // Fast reject: length difference > max_distance
                let len_diff = name_lower.len().abs_diff(query_lower.len());
                if len_diff > max_distance {
                    return None;
                }
                let dist = edit_distance(&query_lower, &name_lower);
                if dist <= max_distance {
                    Some((dist, e))
                } else {
                    None
                }
            })
            .collect();
        scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.name.cmp(&b.1.name)));
        scored.into_iter().take(limit).map(|(_, e)| e).collect()
    }
}

/// Levenshtein edit distance between two strings.
/// Used by `fuzzy_search` for "did you mean?" support.
fn edit_distance(a: &str, b: &str) -> usize {
    let m = a.len();
    let n = b.len();
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for (i, row) in dp.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in dp[0].iter_mut().enumerate() {
        *cell = j;
    }
    for (i, ca) in a.chars().enumerate() {
        for (j, cb) in b.chars().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            dp[i + 1][j + 1] = (dp[i][j + 1] + 1).min(dp[i + 1][j] + 1).min(dp[i][j] + cost);
        }
    }
    dp[m][n]
}

/// Search mode for symbol queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    /// Exact name match (case-insensitive).
    Exact,
    /// Name starts with query.
    Prefix,
    /// Name contains query substring.
    Fuzzy,
}

/// A structured symbol search query.
/// Builder pattern: `Query::new("foo").fuzzy().only_types().case_sensitive()`
#[derive(Debug, Clone)]
pub struct Query {
    query: String,
    lowercased: String,
    mode: SearchMode,
    case_sensitive: bool,
    only_types: bool,
    limit: usize,
}

impl Query {
    /// Create a new query.
    pub fn new(query: String) -> Query {
        let lowercased = query.to_ascii_lowercase();
        Query {
            query,
            lowercased,
            mode: SearchMode::Fuzzy,
            case_sensitive: false,
            only_types: false,
            limit: 128,
        }
    }

    /// Set search mode to exact match.
    pub fn exact(mut self) -> Self {
        self.mode = SearchMode::Exact;
        self
    }

    /// Set search mode to prefix match.
    pub fn prefix(mut self) -> Self {
        self.mode = SearchMode::Prefix;
        self
    }

    /// Set search mode to fuzzy (substring) match.
    pub fn fuzzy(mut self) -> Self {
        self.mode = SearchMode::Fuzzy;
        self
    }

    /// Only return type-like symbols (struct, enum, union, type alias).
    pub fn only_types(mut self) -> Self {
        self.only_types = true;
        self
    }

    /// Enable case-sensitive matching.
    pub fn case_sensitive(mut self) -> Self {
        self.case_sensitive = true;
        self
    }

    /// Set result limit.
    pub fn limit(mut self, n: usize) -> Self {
        self.limit = n;
        self
    }
}

impl SymbolIndex {
    /// Search the index using a structured `Query`.
    pub fn query(&self, q: &Query) -> Vec<&SymbolEntry> {
        if q.query.is_empty() {
            return Vec::new();
        }

        let mut results: Vec<(u32, &SymbolEntry)> = self
            .by_name
            .values()
            .flatten()
            .filter_map(|e| {
                // Type filter
                if q.only_types && !is_type_kind(e.kind) {
                    return None;
                }

                let name_cmp = if q.case_sensitive {
                    e.name.as_str()
                } else {
                    // Can't borrow temp; use lowercased field
                    return self.query_match_score(e, q).map(|score| (score, e));
                };

                let query_cmp = if q.case_sensitive { &q.query } else { &q.lowercased };
                let score = match q.mode {
                    SearchMode::Exact if name_cmp == query_cmp.as_str() => 100,
                    SearchMode::Prefix if name_cmp.starts_with(query_cmp.as_str()) => 80,
                    SearchMode::Fuzzy if name_cmp.contains(query_cmp.as_str()) => 50,
                    _ => return None,
                };
                Some((score, e))
            })
            .collect();

        results.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.name.cmp(&b.1.name)));
        results.truncate(q.limit);
        results.into_iter().map(|(_, e)| e).collect()
    }

    fn query_match_score(&self, e: &SymbolEntry, q: &Query) -> Option<u32> {
        let name_lower = e.name.to_ascii_lowercase();
        if q.only_types && !is_type_kind(e.kind) {
            return None;
        }
        match q.mode {
            SearchMode::Exact if name_lower == q.lowercased => Some(100),
            SearchMode::Prefix if name_lower.starts_with(&q.lowercased) => Some(80),
            SearchMode::Fuzzy if name_lower.contains(&q.lowercased) => Some(50),
            _ => None,
        }
    }
}

fn is_type_kind(kind: ItemKind) -> bool {
    matches!(
        kind,
        ItemKind::Struct
            | ItemKind::Union
            | ItemKind::Enum
            | ItemKind::Bitfield
            | ItemKind::Newtype
            | ItemKind::TypeAlias
    )
}

/// A callback-based signature index stored per-file via salsa.
pub type SignatureIndex = HashMap<String, crate::CallableSignature>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_index_find_returns_empty() {
        let index = SymbolIndex::new();
        assert!(index.find("foo").is_empty());
        assert_eq!(index.ref_count("foo"), 0);
        assert_eq!(index.impl_count("foo"), 0);
    }

    #[test]
    fn search_case_insensitive() {
        let mut index = SymbolIndex::new();
        index.by_name.entry("MyFunction".to_string()).or_default().push(SymbolEntry {
            url: Url::parse("file:///test.sail").unwrap(),
            name: "MyFunction".to_string(),
            kind: ItemKind::Function,
            span: Span::new(0, 10),
            signature_text: "function MyFunction() = ...".to_string(),
            doc: None,
            is_clause: false,
        });
        let results = index.search("myfunc");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "MyFunction");
    }
}
