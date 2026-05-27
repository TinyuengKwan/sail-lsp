//! Symbol search — two-stage find-usages infrastructure.
//! Text search for candidates → semantic resolution for
//! precision.
//!
//! Sail adaptation: stage 1 uses SymbolIndex (O(1) name→files),
//! stage 2 scans ParsedFile.symbol_occurrences in candidate files only.

use base_db::FileId;
use rustc_hash::FxHashMap;

use crate::line_index::TextRange;
use crate::workspace_index::SymbolIndex;
use crate::FileDb;
use parser::Span;
use url::Url;

/// How a symbol is used at a reference site.
/// ```text
/// bitflags! {
///     pub struct ReferenceCategory: u8 {
///         const WRITE = 1 << 0;
///         const READ = 1 << 1;
///         const IMPORT = 1 << 2;
///         const TEST = 1 << 3;
///     }
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ReferenceCategory(u8);

impl ReferenceCategory {
    pub const WRITE: Self = Self(1 << 0);
    pub const READ: Self = Self(1 << 1);
    pub const IMPORT: Self = Self(1 << 2);
    pub const TEST: Self = Self(1 << 3);

    pub fn is_write(self) -> bool {
        self.0 & Self::WRITE.0 != 0
    }

    pub fn is_read(self) -> bool {
        self.0 & Self::READ.0 != 0
    }

    pub fn is_import(self) -> bool {
        self.0 & Self::IMPORT.0 != 0
    }
}

impl std::ops::BitOr for ReferenceCategory {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for ReferenceCategory {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// A single reference occurrence in a file.
#[derive(Debug, Clone)]
pub struct FileReference {
    /// Byte range of the reference.
    pub range: TextRange,
    /// The syntax node of the name/nameref, if available.
    pub name: Option<syntax::SyntaxNodePtr>,
    /// Category: read, write, import, test.
    pub category: ReferenceCategory,
}

/// Result of a usage search across the workspace.
///
/// ```text
/// pub struct UsageSearchResult {
///     pub references: FxHashMap<EditionedFileId, Vec<FileReference>>,
/// }
/// ```
///
/// Keyed by `FileId` (internal concept) instead of `Url`
/// (LSP concept). The Url→FileId conversion happens at the LSP layer.
#[derive(Debug, Clone, Default)]
pub struct UsageSearchResult {
    /// File → references in that file.
    pub references: FxHashMap<FileId, Vec<FileReference>>,
}

impl UsageSearchResult {
    pub fn is_empty(&self) -> bool {
        self.references.values().all(|v| v.is_empty())
    }

    pub fn len(&self) -> usize {
        self.references.values().map(|v| v.len()).sum()
    }

    /// Iterate as `(FileId, &[FileReference])` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (FileId, &[FileReference])> + '_ {
        self.references.iter().map(|(&file_id, refs)| (file_id, refs.as_slice()))
    }

    /// Flatten to a list of `(FileId, TextRange)` pairs.
    pub fn file_ranges(&self) -> impl Iterator<Item = (FileId, TextRange)> + '_ {
        self.references
            .iter()
            .flat_map(|(&file_id, refs)| refs.iter().map(move |r| (file_id, r.range)))
    }
}

/// Scope of a search operation.
/// ```text
/// pub struct SearchScope {
///     entries: FxHashMap<EditionedFileId, Option<TextRange>>,
/// }
/// ```
///
/// Each entry maps a file to an optional range within that file.
/// `None` means "search the whole file"; `Some(range)` means
/// "search only within this range" (for local symbols).
#[derive(Debug, Clone)]
pub struct SearchScope {
    entries: FxHashMap<FileId, Option<TextRange>>,
}

impl SearchScope {
    /// Create a search scope from explicit file entries.
    pub fn new(entries: FxHashMap<FileId, Option<TextRange>>) -> Self {
        Self { entries }
    }

    /// Build an empty search scope.
    pub fn empty() -> Self {
        Self { entries: FxHashMap::default() }
    }

    /// A scope spanning a single file.
    pub fn single_file(file_id: FileId) -> Self {
        let mut entries = FxHashMap::default();
        entries.insert(file_id, None);
        Self { entries }
    }

    /// A scope with a specific range in one file (for local symbols).
    pub fn file_range(file_id: FileId, range: TextRange) -> Self {
        let mut entries = FxHashMap::default();
        entries.insert(file_id, Some(range));
        Self { entries }
    }

    /// A scope spanning multiple files.
    pub fn files(file_ids: &[FileId]) -> Self {
        Self { entries: file_ids.iter().map(|&f| (f, None)).collect() }
    }

    /// Full workspace scope — all known files.
    ///
    /// flat file model.
    pub fn workspace(all_files: impl Iterator<Item = FileId>) -> Self {
        Self { entries: all_files.map(|f| (f, None)).collect() }
    }

    /// All files reachable via $include from a root file.
    ///
    /// defines visibility boundaries (equivalent to crate scope in Rust).
    pub fn include_graph(reachable: impl Iterator<Item = FileId>) -> Self {
        Self { entries: reachable.map(|f| (f, None)).collect() }
    }

    /// Intersection of two scopes.
    pub fn intersection(&self, other: &SearchScope) -> SearchScope {
        let (small, large) = if self.entries.len() <= other.entries.len() {
            (&self.entries, &other.entries)
        } else {
            (&other.entries, &self.entries)
        };
        let entries = small
            .iter()
            .filter_map(|(&file_id, &r1)| {
                let &r2 = large.get(&file_id)?;
                let combined = match (r1, r2) {
                    (None, r) | (r, None) => r,
                    (Some(a), Some(b)) => {
                        Some(TextRange::new(a.start().max(b.start()), a.end().min(b.end())))
                    }
                };
                Some((file_id, combined))
            })
            .collect();
        SearchScope { entries }
    }

    /// Iterate the scope entries.
    pub fn entries(&self) -> impl Iterator<Item = (FileId, Option<TextRange>)> + '_ {
        self.entries.iter().map(|(&file_id, &range)| (file_id, range))
    }

    /// Whether the scope includes a given file.
    pub fn contains(&self, file_id: FileId) -> bool {
        self.entries.contains_key(&file_id)
    }

    /// Number of files in the scope.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the scope is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Two-stage find-usages engine.
///
/// SymbolIndex for stage 1 (candidate files) and
/// symbol_occurrences for stage 2 (precise matches).
///
/// Search is filtered by `SearchScope`; the scope can be set via
/// `in_scope()`.
pub struct FindUsages<'a> {
    index: &'a SymbolIndex,
    /// The name being searched for.
    pub(crate) name: String,
    /// Optional scope restriction. `None` = workspace-wide.
    scope: Option<SearchScope>,
    /// If set, only match occurrences with this target_span.
    target_span: Option<Span>,
}

impl<'a> FindUsages<'a> {
    /// Create a new find-usages search (workspace-wide by default).
    pub fn new(index: &'a SymbolIndex, name: &str) -> Self {
        Self { index, name: name.to_string(), scope: None, target_span: None }
    }

    /// Limit the search to a given `SearchScope`.
    pub fn in_scope(mut self, scope: SearchScope) -> Self {
        self.scope = Some(scope);
        self
    }

    /// Restrict to occurrences pointing to a specific definition span.
    pub fn with_target_span(mut self, span: Span) -> Self {
        self.target_span = Some(span);
        self
    }

    /// Stage 1: Get candidate file URLs from the index.
    pub fn candidate_files(&self) -> Vec<Url> {
        self.index.find(&self.name).iter().map(|e| e.url.clone()).collect()
    }

    /// Run the full search.
    ///
    /// Takes (FileId, &dyn FileDb) pairs — results keyed by FileId.
    pub fn all(&self, files: &[(FileId, &dyn FileDb)]) -> UsageSearchResult {
        let mut result = UsageSearchResult::default();
        for &(file_id, file) in files {
            if let Some(ref scope) = self.scope {
                if !scope.contains(file_id) {
                    continue;
                }
            }
            let refs = self.search_in_file_db(file);
            if !refs.is_empty() {
                result.references.entry(file_id).or_default().extend(refs);
            }
        }
        result
    }

    /// Run the full search via Url-keyed file list (legacy API).
    ///
    /// Wraps `all()` for callers that have `(&Url, FileId, &F)` triples.
    pub fn all_with_urls<F: FileDb>(&self, files: &[(&Url, FileId, &F)]) -> UsageSearchResult {
        let pairs: Vec<(FileId, &dyn FileDb)> =
            files.iter().map(|(_url, fid, file)| (*fid, *file as &dyn FileDb)).collect();
        self.all(&pairs)
    }

    /// Stage 2: Search for occurrences in a single file via FileDb.
    ///
    /// Classifies occurrences into WRITE/READ/IMPORT categories
    /// based on the `DeclRole` from the parser.
    pub fn search_in_file(&self, _url: &Url, file: &dyn FileDb) -> Vec<FileReference> {
        self.search_in_file_db(file)
    }

    /// Stage 2 internal: search via FileDb trait.
    fn search_in_file_db(&self, file: &dyn FileDb) -> Vec<FileReference> {
        let parsed = match file.parsed() {
            Some(p) => p,
            None => return Vec::new(),
        };

        let mut results = Vec::new();
        for occ in &parsed.symbol_occurrences {
            if occ.name != self.name {
                continue;
            }
            if let Some(target) = &self.target_span {
                if let Some(occ_target) = &occ.target_span {
                    if occ_target.start != target.start || occ_target.end != target.end {
                        continue;
                    }
                }
            }
            // Classify the reference category.
            let category = classify_occurrence(occ);
            results.push(FileReference {
                range: base_db::text_range(occ.span.start, occ.span.end),
                name: None,
                category,
            });
        }
        results
    }

    /// Check if at least one usage exists.
    pub fn at_least_one(&self, files: &[(FileId, &dyn FileDb)]) -> bool {
        for &(file_id, file) in files {
            if let Some(ref scope) = self.scope {
                if !scope.contains(file_id) {
                    continue;
                }
            }
            if !self.search_in_file_db(file).is_empty() {
                return true;
            }
        }
        false
    }
}

/// Classify a symbol occurrence into a ReferenceCategory.
///
/// which checks the AST context (assignment lhs, import, etc.).
///
/// Sail's parser provides `DeclRole` on each `SymbolOccurrence`:
/// - Definition/Declaration → WRITE (definition site)
/// - None (usage) → READ (reference site)
fn classify_occurrence(occ: &syntax::parser_lower::SymbolOccurrence) -> ReferenceCategory {
    use syntax::parser_lower::DeclRole;

    match occ.role {
        Some(DeclRole::Definition) | Some(DeclRole::Declaration) => ReferenceCategory::WRITE,
        None => ReferenceCategory::READ,
    }
}

impl crate::defs::Definition {
    /// Create a FindUsages builder pre-configured for this definition.
    ///
    /// ```text
    /// pub fn usages<'a>(self, sema: &'a Semantics<'_, RootDatabase>)
    ///     -> FindUsages<'a>
    /// ```
    ///
    /// Returns a `FindUsages` builder with the definition's name set.
    /// Callers chain `.in_scope()` and `.all()` to run the search.
    pub fn usages<'a>(
        &self,
        db: &dyn hir_def::db::DefDatabase,
        index: &'a SymbolIndex,
    ) -> FindUsages<'a> {
        let name = self.name(db).map(|n| n.to_string()).unwrap_or_default();
        FindUsages::new(index, &name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_scope_single_file() {
        let scope = SearchScope::single_file(FileId::from_raw(0));
        assert!(scope.contains(FileId::from_raw(0)));
        assert!(!scope.contains(FileId::from_raw(1)));
    }

    #[test]
    fn reference_category_bitflags() {
        let mut cat = ReferenceCategory::READ;
        cat |= ReferenceCategory::WRITE;
        assert!(cat.is_read());
        assert!(cat.is_write());
        assert!(!cat.is_import());
    }

    #[test]
    fn usage_search_result_is_empty() {
        let result = UsageSearchResult::default();
        assert!(result.is_empty());
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn usage_search_result_iter() {
        let mut result = UsageSearchResult::default();
        result.references.entry(FileId::from_raw(0)).or_default().push(FileReference {
            range: base_db::text_range(0, 3),
            name: None,
            category: ReferenceCategory::READ,
        });
        result.references.entry(FileId::from_raw(1)).or_default().push(FileReference {
            range: base_db::text_range(10, 13),
            name: None,
            category: ReferenceCategory::WRITE,
        });
        assert_eq!(result.len(), 2);
        assert!(!result.is_empty());
        assert_eq!(result.file_ranges().count(), 2);
    }

    #[test]
    fn search_scope_intersection() {
        let a =
            SearchScope::files(&[FileId::from_raw(0), FileId::from_raw(1), FileId::from_raw(2)]);
        let b =
            SearchScope::files(&[FileId::from_raw(1), FileId::from_raw(2), FileId::from_raw(3)]);
        let c = a.intersection(&b);
        assert!(c.contains(FileId::from_raw(1)));
        assert!(c.contains(FileId::from_raw(2)));
        assert!(!c.contains(FileId::from_raw(0)));
        assert!(!c.contains(FileId::from_raw(3)));
    }
}
