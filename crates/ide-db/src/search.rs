//! Symbol search — two-stage find-usages infrastructure.
//! Text search for candidates → semantic resolution for
//! precision.
//!
//! Sail adaptation: stage 1 uses SymbolIndex (O(1) name→files),
//! stage 2 scans ParsedFile.symbol_occurrences in candidate files only.

use base_db::FileId;
use rustc_hash::FxHashMap;

use crate::line_index::TextRange;
use crate::symbol_index::SymbolIndex;
use crate::FileDb;

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
    /// What is being searched for. rust-analyzer's field, and for its
    /// reason: a search keyed by a name finds every other definition that
    /// spells the same, and a local's binding site is what tells them
    /// apart.
    def: crate::defs::Definition,
    /// The definition's name, resolved once against the database that
    /// holds it — the text the index and the per-file scan match on.
    pub(crate) name: String,
    /// Optional scope restriction. `None` = workspace-wide.
    scope: Option<SearchScope>,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a> FindUsages<'a> {
    /// Create a new find-usages search (workspace-wide by default).
    pub fn new(def: crate::defs::Definition, name: impl Into<String>) -> Self {
        Self { def, name: name.into(), scope: None, _marker: std::marker::PhantomData }
    }

    /// Limit the search to a given `SearchScope`.
    pub fn in_scope(mut self, scope: SearchScope) -> Self {
        self.scope = Some(scope);
        self
    }

    /// What this search is for.
    pub fn definition(&self) -> &crate::defs::Definition {
        &self.def
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

    // There is no `all_with_urls`. It wrapped `all()` for "callers that
    // have `(&Url, FileId, &F)` triples", of which there were none: a
    // search is keyed by `FileId`, and the URL is the server's word for a
    // file, converted at its boundary.

    // There is no `search_in_file(&self, url, file)` either. It took a
    // `_url` it ignored and forwarded to `search_in_file_db`, which is
    // the one every caller reaches.

    /// Stage 2 internal: every mention of this definition in one file.
    ///
    /// Definitions come from the item tree and the bodies, uses from the
    /// callgraph — the three places that know what a name *is*.
    ///
    /// `ide` had a second copy of this walk, reached through
    /// `Semantics::name_mentions`, and this one had no caller at all. They
    /// disagreed about locals: this one matched every `Pat::Bind` that
    /// spelled the name, in every body in the file, so a search for one
    /// `let x` found every other `x` too.
    fn search_in_file_db(&self, file: &dyn FileDb) -> Vec<FileReference> {
        let mut results: Vec<FileReference> = self
            .spans_in_file(file)
            .into_iter()
            .map(|(span, is_decl)| FileReference {
                range: base_db::text_range(span.start, span.end),
                name: None,
                category: if is_decl { ReferenceCategory::WRITE } else { ReferenceCategory::READ },
            })
            .collect();
        results.sort_by_key(|r| (base_db::range_start(r.range), base_db::range_end(r.range)));
        results.dedup_by_key(|r| r.range);
        results
    }

    /// Every mention of this definition in `file`, and whether each one
    /// declares it.
    pub fn spans_in_file(&self, file: &dyn FileDb) -> Vec<(parser::Span, bool)> {
        if !self.def.is_local() {
            if let Some((db, ft)) = file.file_text() {
                if hir::source_order::declarations_for(db, ft).is_none()
                    && base_db::module_graph::file_scope(db, ft.file_id(db)).is_some()
                {
                    let Some(mut origin) = self.def.file_id(db) else { return Vec::new() };
                    let Some(ws) = base_db::WorkspaceFiles::try_get(db) else { return Vec::new() };
                    // A signature/head owns a callable even when its clauses
                    // are defined by a dependent module.
                    if matches!(
                        self.def,
                        crate::defs::Definition::Function(_)
                            | crate::defs::Definition::ValSpec(_)
                            | crate::defs::Definition::ScatteredHead(_)
                            | crate::defs::Definition::ScatteredClause(_)
                            | crate::defs::Definition::EndMarker(_)
                    ) {
                        for &target in hir::visibility::visible_files_for_name(
                            db,
                            ws,
                            origin,
                            hir::Name::new(&self.name),
                        ) {
                            if hir::def_query::file_item_tree(db, target).as_ref().is_some_and(
                                |tree| {
                                    tree.top_level_items().iter().any(|id| {
                                        id.name(tree).as_str() == self.name
                                            && matches!(
                                                id.def_kind(tree),
                                                hir::DefKind::ValSpec | hir::DefKind::ScatteredHead
                                            )
                                    })
                                },
                            ) {
                                origin = target.file_id(db);
                                break;
                            }
                        }
                    }
                    let candidates = hir::visibility::visible_files_for_name(
                        db,
                        ws,
                        ft.file_id(db),
                        hir::Name::new(&self.name),
                    );
                    if !candidates.iter().any(|&target| target.file_id(db) == origin) {
                        return Vec::new();
                    }
                }
            }
        }
        let mut spans = file
            .file_text()
            .map(|(db, ft)| {
                hir::Semantics::new(db).name_mentions(
                    db,
                    ft,
                    &self.name,
                    self.def.binding_span(),
                    !self.def.is_local(),
                )
            })
            .unwrap_or_default();

        // HIR bodies omit declaration type syntax. Include lexical candidates
        // there too, then let the shared identity classifier below decide
        // whether each occurrence denotes this definition. This also covers
        // scattered clause/end references without treating them as new names.
        if !self.def.is_local() {
            if let Some((db, ft)) = file
                .file_text()
                .filter(|(db, ft)| hir::source_order::declarations_for(*db, *ft).is_some())
            {
                if let Some(root) = syntax::parse_query::semantic_parse_file(db, ft).syntax_node() {
                    spans.extend(
                        root.descendants_with_tokens()
                            .filter_map(|t| t.into_token())
                            .filter(|t| {
                                t.kind() == syntax::SyntaxKind::IDENT
                                    && t.text() == self.name
                                    && !t.parent_ancestors().any(|n| {
                                        matches!(
                                            n.kind(),
                                            syntax::SyntaxKind::ATTRIBUTE
                                                | syntax::SyntaxKind::DIRECTIVE_DEF
                                        )
                                    })
                            })
                            .map(|t| {
                                (
                                    parser::Span::new(
                                        usize::from(t.text_range().start()),
                                        usize::from(t.text_range().end()),
                                    ),
                                    false,
                                )
                            }),
                    );
                }
            }
        }

        // A name is never empty, so a zero-width span is a missing one —
        // some expressions still lower with no recorded position, and
        // reporting them puts a reference at the top of the file.
        spans.retain(|(span, _)| span.start < span.end);

        // Locals need the same identity check: their lexical scope also covers
        // callable names and bitfield labels, which are separate namespaces.
        if let Some((db, _)) = file
            .file_text()
            .filter(|(db, ft)| hir::source_order::declarations_for(*db, *ft).is_some())
        {
            let expected = self.def.canonical(db);
            spans.retain(|(span, _)| {
                crate::defs::Definition::classify_at(file, file.position_at(span.start))
                    .is_some_and(|actual| actual.canonical(db) == expected)
            });
        }

        // A type variable in a signature — `val f : forall ('n). bits('n)`
        // — has no HIR representation at all: the item tree records the
        // signature's text, not the variables in it, so the occurrence
        // scrape is the only thing that knows where they are.
        //
        // Only for those. Firing whenever the HIR found nothing made a file
        // that genuinely has no mention of the name answer with every token
        // that spells it — which is how a top-level `foo` picked up an
        // unrelated `let foo` in another file.
        if spans.is_empty() && self.name.starts_with('\'') {
            if let Some(parsed) = file.parsed() {
                for occ in &parsed.symbol_occurrences {
                    if occ.name == self.name {
                        spans.push((occ.span, occ.role.is_some()));
                    }
                }
            }
        }
        spans
    }

    // And no `at_least_one`. It short-circuits "is this used anywhere",
    // which is what an unused-definition lint would want — and there is
    // no such lint here. Adding one is a feature, not a repair; it can
    // come back with its caller.
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
    pub fn usages<'a>(&self, db: &dyn salsa::Database, index: &'a SymbolIndex) -> FindUsages<'a> {
        let name = self.name(db).map(|n| n.to_string()).unwrap_or_default();
        let _ = index;
        FindUsages::new(self.clone(), name)
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
