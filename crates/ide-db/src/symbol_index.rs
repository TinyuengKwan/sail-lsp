//! Workspace-wide symbol index.
//!
//! Eliminates O(n) `all_salsa_files()` scans by pre-building a name->location
//! index after workspace scan. Stored in `Arc` for cheap snapshot sharing.
//! Uses HashMap (exact-match is the common case for Sail workspaces).

use std::collections::HashMap;

use parser::Span;

use hir::DefKind;

/// Per-symbol entry in the workspace index.
/// Derived from `ItemTreeEntry` — contains only what handlers need for lookups.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolEntry {
    /// File where this symbol is defined.
    ///
    /// A `FileId`, not a `Url`: the index is built by a salsa query now,
    /// and salsa knows files by id. The URL is the editor's word for a
    /// file and belongs at the boundary that talks to it.
    pub file_id: base_db::FileId,
    /// Symbol name.
    pub name: String,
    /// Item kind (Function, Struct, Enum, ValSpec, etc.)
    pub kind: DefKind,
    /// Byte span of the definition in source.
    pub span: Span,
    /// Byte span of just the declared name, for cursor placement.
    pub name_span: Span,
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
    /// file → the symbol names it defines.
    by_file: HashMap<base_db::FileId, Vec<String>>,
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
    pub fn build(
        db: &dyn salsa::Database,
        files: impl IntoIterator<Item = (base_db::FileId, base_db::FileText)>,
    ) -> Self {
        let mut index = Self::new();
        for (file_id, ft) in files {
            index.insert_file(file_id, &FileSymbols::of(db, file_id, ft));
        }
        index
    }

    /// Merge one file's symbols in, replacing whatever it had before.
    pub fn insert_file(&mut self, file_id: base_db::FileId, symbols: &FileSymbols) {
        self.remove_file(file_id);
        let mut names = Vec::with_capacity(symbols.entries.len());
        for entry in &symbols.entries {
            names.push(entry.name.clone());
            self.by_name.entry(entry.name.clone()).or_default().push(entry.clone());
        }
        for (name, n) in &symbols.ref_counts {
            *self.ref_counts.entry(name.clone()).or_insert(0) += n;
        }
        for (name, n) in &symbols.impl_counts {
            *self.impl_counts.entry(name.clone()).or_insert(0) += n;
        }
        self.by_file.insert(file_id, names);
    }

    /// Add/replace all entries for a single file.
    fn add_file_from(
        &mut self,
        file_id: base_db::FileId,
        item_tree: Option<&hir::ItemTree>,
        callgraph: Option<&hir::callgraph::CallGraph>,
    ) {
        // Remove old entries for this file first
        self.remove_file(file_id);

        let mut file_names = Vec::new();

        // Extract entries from ItemTree (definitions + declarations)
        if let Some(item_tree) = item_tree {
            for &id in item_tree.top_level_items() {
                let name = id.name(item_tree).as_str().to_string();
                let kind = id.def_kind(item_tree);
                file_names.push(name.clone());
                self.by_name.entry(name.clone()).or_default().push(SymbolEntry {
                    file_id,
                    name: name.clone(),
                    kind,
                    span: id.span(item_tree),
                    name_span: id.name_span(item_tree),
                    signature_text: id.signature(item_tree).to_string(),
                    doc: id.doc(item_tree).map(|s| s.to_string()),
                    is_clause: id.is_clause(item_tree),
                });
                // A struct's or bitfield's fields are declarations too, and
                // navigation asks for them by name: `struct { variety = .. }`
                // and `v[SSI]` name a field with nothing else to go on. They
                // are not top-level items, so the walk above cannot reach
                // them; rust-analyzer carries fields in its symbol index for
                // the same reason.
                let owner = name;
                let member_kind = match kind {
                    DefKind::Union | DefKind::Enum => DefKind::Variant,
                    _ => DefKind::Field,
                };
                for field in id.fields(item_tree) {
                    let fname = field.name.as_str().to_string();
                    file_names.push(fname.clone());
                    self.by_name.entry(fname.clone()).or_default().push(SymbolEntry {
                        file_id,
                        name: fname,
                        kind: member_kind,
                        span: field.name_span,
                        name_span: field.name_span,
                        signature_text: if field.type_text.is_empty() {
                            format!("{owner}.{}", field.name.as_str())
                        } else {
                            format!("{}: {}", field.name.as_str(), field.type_text)
                        },
                        doc: None,
                        is_clause: false,
                    });
                }
                // Same reasoning for a scattered clause: `enum clause
                // extension = Ext_S` is indexed under `extension`, and every
                // use site writes `Ext_S`.
                if let Some((member, member_span)) = id.scattered_member(item_tree) {
                    file_names.push(member.to_string());
                    self.by_name.entry(member.to_string()).or_default().push(SymbolEntry {
                        file_id,
                        name: member.to_string(),
                        kind: DefKind::Variant,
                        span: id.span(item_tree),
                        name_span: member_span,
                        signature_text: id.signature(item_tree).to_string(),
                        doc: id.doc(item_tree).map(|s| s.to_string()),
                        is_clause: true,
                    });
                }
            }
        }

        // Call sites, from the callgraph built out of the lowered bodies.
        if let Some(graph) = callgraph {
            for site in graph.call_sites_in_file() {
                *self.ref_counts.entry(site.callee.clone()).or_insert(0) += 1;
            }
        }
        // Implementations, from the item tree. A `function clause` of a
        // scattered definition is one.
        if let Some(tree) = item_tree {
            for &id in tree.top_level_items() {
                if matches!(
                    id.def_kind(tree),
                    hir::DefKind::Function | hir::DefKind::Mapping | hir::DefKind::ScatteredClause
                ) {
                    *self.impl_counts.entry(id.name(tree).to_string()).or_insert(0) += 1;
                }
            }
        }

        self.by_file.insert(file_id, file_names);
    }

    /// Remove all entries for a file (for incremental update or deletion).
    pub fn remove_file(&mut self, file_id: base_db::FileId) {
        if let Some(names) = self.by_file.remove(&file_id) {
            for name in &names {
                if let Some(entries) = self.by_name.get_mut(name) {
                    entries.retain(|e| e.file_id != file_id);
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

fn is_type_kind(kind: DefKind) -> bool {
    matches!(
        kind,
        DefKind::Struct
            | DefKind::Union
            | DefKind::Enum
            | DefKind::Bitfield
            | DefKind::Newtype
            | DefKind::TypeAlias
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
            file_id: base_db::FileId::from_raw(0),
            name: "MyFunction".to_string(),
            kind: DefKind::Function,
            span: Span::new(0, 10),
            name_span: Span::new(9, 19),
            signature_text: "function MyFunction() = ...".to_string(),
            doc: None,
            is_clause: false,
        });
        let results = index.search("myfunc");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "MyFunction");
    }
}

/// One file's symbols, as a salsa query result.
///
/// rust-analyzer's symbol index is a salsa query per source root, built
/// from one per file. Ours was a `HashMap` in `GlobalState`, updated by
/// hand from a dirty-file list and **deep-cloned on every batch** — 8.9 ms
/// on sail-riscv, on the main loop's thread, per settled edit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileSymbols {
    pub entries: Vec<SymbolEntry>,
    pub ref_counts: Vec<(String, usize)>,
    pub impl_counts: Vec<(String, usize)>,
}

impl FileSymbols {
    /// What `file` declares, calls and implements.
    pub fn of(db: &dyn salsa::Database, file_id: base_db::FileId, ft: base_db::FileText) -> Self {
        let item_tree = hir::def_query::file_item_tree(db, ft);
        let callgraph = hir::def_query::callgraph(db, ft);
        let mut index = SymbolIndex::new();
        index.add_file_from(
            file_id,
            item_tree.as_deref(),
            callgraph.as_ref().map(|cg| cg.0.as_ref()),
        );
        let mut ref_counts: Vec<(String, usize)> = index.ref_counts.into_iter().collect();
        let mut impl_counts: Vec<(String, usize)> = index.impl_counts.into_iter().collect();
        ref_counts.sort();
        impl_counts.sort();
        let mut entries: Vec<SymbolEntry> = index.by_name.into_values().flatten().collect();
        entries.sort_by(|a, b| {
            (&a.name, a.span.start, a.span.end).cmp(&(&b.name, b.span.start, b.span.end))
        });
        Self { entries, ref_counts, impl_counts }
    }
}
