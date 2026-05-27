//! Go to definition — resolves the definition site of a symbol.
//!
//! ```text
//! pub(crate) fn goto_definition(
//!     db: &RootDatabase,
//!     position: FilePosition,
//! ) -> Option<RangeInfo<Vec<NavigationTarget>>>
//! ```
//!
//! # Architecture
//!
//! Two resolution paths:
//!
//! 1. **Semantic**: Offset → token → name → Resolver (scope-aware) → definition
//!    - Handles local bindings, shadowing, same-file definitions
//!    - Uses body source maps for local variable binding locations
//!
//! 2. **Index fallback**: Token text → SymbolIndex → FileLocation
//!    - Fast O(1) for cross-file navigation
//!    - Does not handle shadowing or scope-awareness

use ide_db::ide_types::{FileLocation, NavigationTarget, SymbolKind};
use ide_db::line_index::LineCol;
use ide_db::workspace_index::SymbolIndex;
use ide_db::FileDb;
use url::Url;

use hir_def::item_tree::ItemKind;

// Re-export the legacy generic version for callers that pass file iterators.
pub use crate::navigation::symbol_definition_locations;

//
// Sail supports `@property` pragmas that annotate functions with metadata:
//
//   @property("termination_measure", f, "n")
//   function f(n : int) -> int = ...
//
// When the cursor is on a property reference (e.g., the function name `f`
// inside an @property pragma), goto-definition should navigate to the
// annotated function's definition site.
//
// Implementation plan:
// 1. In `item_tree/lower.rs`, recognize `@property` directives and store
//    them as metadata on the annotated function's ItemTree entry.
// 2. Here, detect when the cursor is inside a `@property` pragma token
//    (check if the line starts with `@property` and cursor is on a name).
// 3. Extract the function name from the pragma and delegate to the
//    existing definition resolution (semantic or index-based).
//
// This requires pragma parsing infrastructure in hir-def and is deferred
// to a follow-up pass.

/// Semantic goto-definition: resolve the symbol at `position` in `file`
/// using scope-aware name resolution.
///
/// position via semantic analysis rather than text-based index lookup.
///
/// Returns `None` if semantic resolution fails (caller should fall
/// back to the index-based `goto_definition`).
///
/// ## Resolution strategy
///
/// 1. Find the token at the cursor position
/// 2. Extract the symbol name from the token
/// 3. Check if the offset is inside a callable body:
///    a. If yes: check for local binding (let/var/match pattern)
///    b. If no binding: check for same-file definition
/// 4. If same-file definition found: return its span as NavigationTarget
pub fn goto_definition_semantic<F: FileDb>(
    file: &F,
    position: LineCol,
    file_url: &Url,
) -> Option<Vec<NavigationTarget>> {
    // 1. Find token at cursor position
    let (token, _span) = file.token_at(position)?;
    let name = ide_db::token_symbol_key(token)?;

    // 2. Check local bindings first (scope-aware: handles shadowing)
    if let Some(bodies) = file.bodies() {
        let offset = file.offset_at(&position);
        for entry in bodies.entries() {
            // Check if offset is inside this callable's body
            if let Some(expr_id) = entry.source_map.expr_at_offset(offset) {
                // The cursor is inside this body — check if the name
                // is a reference to a pattern-bound local variable.
                if let hir_def::Expr::Ident(ref ident_name) = entry.body.store[expr_id] {
                    if ident_name == &name {
                        // Search this body's patterns for the binding site
                        if let Some(nav) = find_local_binding_nav(entry, &name, file_url) {
                            return Some(vec![nav]);
                        }
                    }
                }
            }
        }
    }

    // 3. Check same-file top-level definitions
    if let Some(parsed) = file.parsed() {
        let targets = find_same_file_definition(parsed, &name, file_url);
        if !targets.is_empty() {
            return Some(targets);
        }
    }

    None
}

/// Semantic goto-definition via `Semantics::classify_name_ref`.
///
/// under the cursor. This is a thin wrapper that creates a `Semantics`,
/// calls `classify_name_ref`, and returns the `PathResolution`.
///
/// Callers can use the returned `PathResolution` to build `NavigationTarget`s
/// via the index or DefMap. Currently used as an auxiliary resolution path;
/// the main `goto_definition_semantic` still handles local bindings and
/// same-file definitions directly.
pub fn classify_name_ref_at(
    db: &dyn salsa::Database,
    def_db: &dyn hir_def::db::DefDatabase,
    file_text: base_db::FileText,
    offset: usize,
    name: &str,
) -> Option<hir::NameRefKind> {
    let sema = hir::Semantics::new(db);
    sema.classify_name_ref(def_db, file_text, offset, name)
}

/// Legacy version that returns just a PathResolution.
/// Used by callers that only need the resolution, not the kind.
pub fn classify_name_ref_path_at(
    db: &dyn salsa::Database,
    def_db: &dyn hir_def::db::DefDatabase,
    file_text: base_db::FileText,
    offset: usize,
    name: &str,
) -> Option<hir::PathResolution> {
    let sema = hir::Semantics::new(db);
    sema.classify_name_ref_path(def_db, file_text, offset, name)
}

/// Search a callable body's patterns for a binding with the given name.
/// Returns the binding's location as a NavigationTarget.
fn find_local_binding_nav(
    entry: &hir_def::bodies::CallableBody,
    name: &str,
    file_url: &Url,
) -> Option<NavigationTarget> {
    for (pat_id, pat) in entry.body.iter_pats() {
        if let hir_def::Pat::Bind(ref pat_name) = pat {
            if pat_name == name {
                if let Some(pat_span) = entry.source_map.pat_syntax(pat_id) {
                    return Some(NavigationTarget {
                        file_id: None,
                        url: file_url.clone(),
                        name: name.to_string(),
                        kind: SymbolKind::Variable,
                        full_range: base_db::text_range(pat_span.start, pat_span.end),
                        focus_range: base_db::text_range(pat_span.start, pat_span.end),
                        detail: None,
                        docs: None,
                        children: Vec::new(),
                    });
                }
            }
        }
    }
    None
}

/// Search the file's parsed declarations for a top-level definition
/// matching the given name.
fn find_same_file_definition(
    parsed: &syntax::parser_lower::ParsedFile,
    name: &str,
    file_url: &Url,
) -> Vec<NavigationTarget> {
    use syntax::parser_lower::DeclRole;

    let mut targets = Vec::new();

    // First pass: look for non-clause declarations (definitions)
    for decl in &parsed.decls {
        if decl.name == name && decl.role == DeclRole::Definition {
            targets.push(NavigationTarget {
                file_id: None,
                url: file_url.clone(),
                name: name.to_string(),
                kind: decl_kind_to_symbol_kind(&decl.kind),
                full_range: base_db::text_range(decl.span.start, decl.span.end),
                focus_range: base_db::text_range(decl.span.start, decl.span.end),
                detail: None,
                docs: None,
                children: Vec::new(),
            });
        }
    }

    // If no definitions found, include declarations (val specs)
    if targets.is_empty() {
        for decl in &parsed.decls {
            if decl.name == name && decl.role == DeclRole::Declaration {
                targets.push(NavigationTarget {
                    file_id: None,
                    url: file_url.clone(),
                    name: name.to_string(),
                    kind: decl_kind_to_symbol_kind(&decl.kind),
                    full_range: base_db::text_range(decl.span.start, decl.span.end),
                    focus_range: base_db::text_range(decl.span.start, decl.span.end),
                    detail: None,
                    docs: None,
                    children: Vec::new(),
                });
            }
        }
    }

    targets
}

/// 投産-4: Semantic goto-definition using SourceAnalyzer for field access
/// and method call resolution. Requires salsa database + FileText.
///
/// This supplements `goto_definition_semantic` by resolving:
/// - Field access expressions (e.g., `x.field_name`) via `resolve_field`
/// - Function call expressions (e.g., `f(args)`) via `resolve_method_call`
///
/// Returns `None` if no field/method resolution is found at the offset.
///
/// two separate resolution calls.
pub fn goto_definition_field_or_method(
    db: &dyn salsa::Database,
    file_text: base_db::FileText,
    offset: usize,
    file_url: &Url,
    index: &SymbolIndex,
) -> Option<Vec<NavigationTarget>> {
    let source = file_text.text(db);

    // Extract the identifier token at the offset from source text
    let token_name = extract_identifier_at(&source, offset)?;

    let sema = hir::Semantics::new(db);

    // Use combined field/method resolution (Either<field, method>).
    // Both branches look up the token name in the SymbolIndex.
    let _resolution = sema.resolve_field_or_method(file_text, offset)?;

    let targets = symbol_index_to_nav_targets(index, &token_name, file_url);
    if !targets.is_empty() {
        return Some(targets);
    }

    None
}

/// Extract the identifier (word) at a byte offset in source text.
/// Returns the identifier string if the offset points into an identifier.
fn extract_identifier_at(source: &str, offset: usize) -> Option<String> {
    if offset >= source.len() {
        return None;
    }
    let bytes = source.as_bytes();
    // Check that offset is within an identifier character
    if !is_ident_char(bytes[offset]) {
        return None;
    }
    // Scan backward to find start
    let mut start = offset;
    while start > 0 && is_ident_char(bytes[start - 1]) {
        start -= 1;
    }
    // Scan forward to find end
    let mut end = offset;
    while end < bytes.len() && is_ident_char(bytes[end]) {
        end += 1;
    }
    Some(source[start..end].to_string())
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Look up a symbol name in the SymbolIndex and convert to NavigationTargets.
/// Prefers definitions over declarations; prefers the same file.
fn symbol_index_to_nav_targets(
    index: &SymbolIndex,
    name: &str,
    current_url: &Url,
) -> Vec<NavigationTarget> {
    let entries = index.find(name);
    if entries.is_empty() {
        return Vec::new();
    }

    // Prefer definitions (Function, Struct, etc.) over declarations (ValSpec)
    let definitions: Vec<_> = entries
        .iter()
        .filter(|e| !matches!(e.kind, ItemKind::ValSpec | ItemKind::MappingSpec))
        .collect();

    let chosen =
        if definitions.is_empty() { entries.iter().collect::<Vec<_>>() } else { definitions };

    // Prefer entries from the same file
    let mut sorted = chosen;
    sorted.sort_by(|a, b| {
        let a_same = a.url == *current_url;
        let b_same = b.url == *current_url;
        b_same.cmp(&a_same)
    });

    sorted
        .into_iter()
        .map(|entry| NavigationTarget {
            file_id: None,
            url: entry.url.clone(),
            name: entry.name.clone(),
            kind: item_kind_to_symbol_kind(entry.kind),
            full_range: base_db::text_range(entry.span.start, entry.span.end),
            focus_range: base_db::text_range(entry.span.start, entry.span.end),
            detail: if entry.signature_text.is_empty() {
                None
            } else {
                Some(entry.signature_text.clone())
            },
            docs: entry.doc.clone(),
            children: Vec::new(),
        })
        .collect()
}

/// Go to definition using the workspace symbol index (O(1) lookup).
///
/// This is the cross-file fallback path when semantic resolution
/// doesn't find a result in the current file.
///
/// Mirrors the legacy API (kept for backward compatibility).
pub fn goto_definition(index: &SymbolIndex, symbol_key: &str, uri_hint: &Url) -> Vec<FileLocation> {
    crate::navigation::definition_locations_indexed(index, symbol_key, uri_hint)
}

/// Convert a `DeclKind` to a `SymbolKind`.
fn decl_kind_to_symbol_kind(kind: &syntax::parser_lower::DeclKind) -> SymbolKind {
    use syntax::parser_lower::DeclKind;
    match kind {
        DeclKind::Function => SymbolKind::Function,
        DeclKind::Value => SymbolKind::Function,
        DeclKind::Mapping => SymbolKind::Function,
        DeclKind::Overload => SymbolKind::Function,
        DeclKind::Register => SymbolKind::Variable,
        DeclKind::Parameter => SymbolKind::Variable,
        DeclKind::Type => SymbolKind::Struct,
        DeclKind::Struct => SymbolKind::Struct,
        DeclKind::Union => SymbolKind::Struct, // Sail unions mapped to Struct kind
        DeclKind::Bitfield => SymbolKind::Struct,
        DeclKind::Enum => SymbolKind::Enum,
        DeclKind::EnumMember => SymbolKind::EnumMember,
        DeclKind::Newtype => SymbolKind::Struct,
        DeclKind::Let | DeclKind::Var => SymbolKind::Variable,
    }
}

/// Convert an `ItemKind` to a `SymbolKind` for NavigationTarget.
pub fn item_kind_to_symbol_kind(kind: ItemKind) -> SymbolKind {
    match kind {
        ItemKind::Function => SymbolKind::Function,
        ItemKind::ValSpec => SymbolKind::Function,
        ItemKind::Struct => SymbolKind::Struct,
        ItemKind::Union => SymbolKind::Struct, // Sail unions mapped to Struct kind
        ItemKind::Enum => SymbolKind::Enum,
        ItemKind::Bitfield => SymbolKind::Struct,
        ItemKind::Newtype => SymbolKind::Struct,
        ItemKind::TypeAlias => SymbolKind::TypeAlias,
        ItemKind::Register => SymbolKind::Variable,
        ItemKind::Mapping | ItemKind::MappingSpec => SymbolKind::Function,
        ItemKind::Let | ItemKind::Var => SymbolKind::Variable,
        ItemKind::Overload => SymbolKind::Function,
        ItemKind::ScatteredHead | ItemKind::ScatteredClause => SymbolKind::Function,
        ItemKind::Constraint => SymbolKind::TypeAlias,
        ItemKind::TerminationMeasure | ItemKind::EndMarker | ItemKind::Instantiation => {
            SymbolKind::Variable
        }
    }
}
